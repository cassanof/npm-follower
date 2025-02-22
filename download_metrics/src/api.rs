use chrono::NaiveDate;
use postgres_db::{download_metrics::DownloadMetric, packages::Package};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::{Mutex, Semaphore},
    task::JoinHandle,
};

use crate::make_download_metric;

/// API wrapper that handles the rate limiting
#[derive(Debug, Clone)]
pub struct API {
    pub pool: Arc<Semaphore>,
    pub pool_size: u32,
    pub rl_lock: Arc<Mutex<()>>,
    pub client: reqwest::Client,
}

pub type QueryTaskHandle = JoinHandle<(Result<DownloadMetric, ApiError>, Package)>;
pub type BulkQueryTaskHandle = JoinHandle<(Result<Vec<DownloadMetric>, ApiError>, Vec<Package>)>;

impl API {
    pub fn new(pool_size: u32) -> API {
        API {
            pool: Arc::new(Semaphore::new(pool_size as usize)),
            pool_size,
            rl_lock: Arc::new(Mutex::new(())),
            client: reqwest::Client::new(),
        }
    }

    pub fn spawn_bulk_query_task(
        self,
        pkgs: Vec<Package>,
        lbound: chrono::NaiveDate,
        rbound: chrono::NaiveDate,
    ) -> BulkQueryTaskHandle {
        tokio::spawn(async move {
            let thunk = async {
                let api_result = self.bulkquery_npm_metrics(&pkgs, &lbound, &rbound).await?;
                let mut metrics = Vec::new();
                for (pkg_name, result) in api_result {
                    if let Some(result) = result {
                        let pkg = pkgs.iter().find(|p| p.name == pkg_name).unwrap(); // yeah, this is bad
                        metrics.push(make_download_metric(pkg, &result).await?);
                    }
                }
                Ok(metrics)
            };
            (thunk.await, pkgs)
        })
    }

    pub fn spawn_query_task(
        self,
        pkg: Package,
        lbound: chrono::NaiveDate,
        rbound: chrono::NaiveDate,
    ) -> QueryTaskHandle {
        tokio::spawn(async move {
            let thunk = async {
                let api_result = self.query_npm_metrics(&pkg, &lbound, &rbound).await?;
                make_download_metric(&pkg, &api_result).await
            };
            (thunk.await, pkg)
        })
    }

    /// Sends a query with reqwest, taking account of the rate limit pool.
    async fn send_query(&self, query: &str) -> Result<(Option<String>, bool), ApiError> {
        {
            self.rl_lock.lock().await;
        }
        let permit = self.pool.acquire().await.unwrap();

        let max_retries = 10;
        for retry_count in 0..max_retries {
            let resp_result = self.client.get(query).send().await;
            let resp = match resp_result {
                Err(err) => {
                    let err_str = format!("{}", err);
                    if err_str.contains("connection closed before message completed")
                        && retry_count < max_retries - 1
                    {
                        // sleep for 10 seconds
                        println!(
                            "received bad response: {:?}, sleeping for 10 seconds before retry",
                            Err::<(), _>(err)
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                        continue;
                    } else {
                        return Err(ApiError::Reqwest(err));
                    }
                }
                Ok(resp) => resp,
            };

            if resp.status() == 429 {
                drop(permit);
                return Ok((None, true));
            } else {
                let text = resp.text().await?;
                let trimmed = text.trim();
                if trimmed.is_empty()
                    || trimmed == "Internal Server Error"
                    || trimmed.contains("524 Origin Time-out")
                    || trimmed.contains("502 Bad Gateway")
                {
                    // sleep for 10 seconds
                    println!(
                        "received bad response: {}, sleeping for 10 seconds before retry",
                        text
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                } else {
                    drop(permit);
                    return Ok((Some(text), false));
                }
            }
        }

        drop(permit);
        Ok((None, true))
    }

    /// When we are getting rate limited, this function is called, and it handles the sleep.
    /// The idea is to get only one thread to sleep and all the others to return.
    async fn handle_rate_limit(&self) {
        let lock = match self.rl_lock.try_lock() {
            Ok(l) => l,
            Err(_) => return,
        };
        let time = std::time::Duration::from_secs(1200);
        println!(
            "Rate-limit hit, sleeping for {} minutes",
            (time.as_secs() as f64) / 60.0
        );
        tokio::time::sleep(time).await;
        drop(lock);
    }

    /// Abstraction to remove duplicate code between the bulk and single query functions.
    async fn query_abstraction<T, R: for<'a> Deserialize<'a>, M>(
        &self,
        thing_to_query: &T,
        formatter: fn(&T) -> String,
        merger: M,
        lbound: &NaiveDate,
        rbound: &NaiveDate,
    ) -> Result<R, ApiError>
    where
        M: Fn(&mut R, R),
    {
        let delta = chronoutil::RelativeDuration::years(1);
        // we are going to merge the results of multiple queries into one
        let mut api_result = None;
        // we can only query 365 days at a time, so we must split the query into multiple requests
        let mut rel_lbound = *lbound;
        let rule = chronoutil::DateRule::new(rel_lbound + delta, delta);
        for mut rel_rbound in rule {
            if rel_lbound > *rbound {
                break;
            }

            if rel_rbound > *rbound {
                // we must not query past the rbound
                rel_rbound = *rbound;
            }

            let query_thing = formatter(thing_to_query);

            // println!(
            //     "Querying {} from {} to {}",
            //     query_thing, rel_lbound, rel_rbound
            // );

            let query = format!(
                "https://api.npmjs.org/downloads/range/{}:{}/{}",
                rel_lbound, rel_rbound, query_thing
            );

            println!("Querying {}", query);
            let (resp_text, is_rate_limited) = self.send_query(&query).await?;

            if is_rate_limited {
                self.handle_rate_limit().await;
                return Err(ApiError::RateLimit);
            }
            let text = resp_text.unwrap();

            let result: R = parse_resp(text)?;

            if api_result.is_none() {
                api_result = Some(result);
            } else {
                let mut new_api_result = api_result.unwrap();
                merger(&mut new_api_result, result);
                api_result = Some(new_api_result);
            }

            rel_lbound = rel_rbound + chronoutil::RelativeDuration::days(1);
        }
        api_result.ok_or_else(|| panic!("api_result is None"))
    }

    /// Performs a regular query in npm download metrics api for each package given
    async fn query_npm_metrics(
        &self,
        pkg: &Package,
        lbound: &NaiveDate,
        rbound: &NaiveDate,
    ) -> Result<ApiResult, ApiError> {
        fn formatter(pkg: &Package) -> String {
            pkg.name.clone()
        }
        fn merger(api_result: &mut ApiResult, result: ApiResult) {
            for dl in result.downloads {
                api_result.downloads.push(dl);
            }
            api_result.end = result.end;
        }
        self.query_abstraction(pkg, formatter, merger, lbound, rbound)
            .await
    }

    /// Performs a bulk query in npm download metrics api for each package given
    /// Can only handle 128 packages at a time, and can't query scoped packages
    async fn bulkquery_npm_metrics(
        self,
        pkgs: &Vec<Package>,
        lbound: &NaiveDate,
        rbound: &NaiveDate,
    ) -> Result<BulkApiResult, ApiError> {
        // The request CAN NOT contain the pattern 'javascript.*window.*onerror' or 'javascript.*onerror.*window'
        // because cloudflare will block that.
        // This is a workaround for that.

        let javascript_pkg_indices: Vec<_> = pkgs
            .iter()
            .enumerate()
            .filter_map(|(i, pkg)| {
                if pkg.name.contains("javascript") {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        let window_pkg_indices: Vec<_> = pkgs
            .iter()
            .enumerate()
            .filter_map(|(i, pkg)| {
                if pkg.name.contains("window") {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        let onerror_pkg_indices: Vec<_> = pkgs
            .iter()
            .enumerate()
            .filter_map(|(i, pkg)| {
                if pkg.name.contains("onerror") {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        if !javascript_pkg_indices.is_empty()
            && !window_pkg_indices.is_empty()
            && !onerror_pkg_indices.is_empty()
        {
            let javascript_pkgs = javascript_pkg_indices
                .iter()
                .map(|i| pkgs[*i].clone())
                .collect::<Vec<_>>();
            let non_javascript_pkgs = pkgs
                .iter()
                .filter(|pkg| !pkg.name.contains("javascript"))
                .cloned()
                .collect::<Vec<_>>();

            let mut map = self
                .real_bulkquery_npm_metrics(&javascript_pkgs, lbound, rbound)
                .await?;
            let other_map = self
                .real_bulkquery_npm_metrics(&non_javascript_pkgs, lbound, rbound)
                .await?;

            for (pkg_name, api_result) in other_map {
                map.insert(pkg_name, api_result);
            }
            return Ok(map);
        }

        self.real_bulkquery_npm_metrics(pkgs, lbound, rbound).await
    }

    async fn real_bulkquery_npm_metrics(
        &self,
        pkgs: &Vec<Package>,
        lbound: &NaiveDate,
        rbound: &NaiveDate,
    ) -> Result<BulkApiResult, ApiError> {
        // We can't do a bulk query with only 1 package, since thats the same
        // as a regular query, and then it won't be deserialized correctly.
        // Instead, handle that case as a normal query.
        if pkgs.len() == 1 {
            let pkg = &pkgs[0];
            let api_result = self.query_npm_metrics(pkg, lbound, rbound).await?;
            let mut map = HashMap::new();
            map.insert(pkg.name.clone(), Some(api_result));
            return Ok(map);
        }

        assert!(2 <= pkgs.len());
        assert!(pkgs.len() <= 128);

        #[allow(clippy::ptr_arg)]
        fn formatter(pkgs: &Vec<Package>) -> String {
            pkgs.iter()
                .map(|pkg| pkg.name.to_string())
                .collect::<Vec<String>>()
                .join(",")
        }

        let merger = |api_result: &mut BulkApiResult, result: BulkApiResult| {
            // not_founds is just for printing
            let mut not_founds = String::new();
            for pkg in pkgs {
                let pkg_name = pkg.name.to_string();
                let pkg_res = match result.get(&pkg_name) {
                    Some(Some(p)) => p,
                    _ => {
                        not_founds.push_str(&pkg_name);
                        not_founds.push_str(", ");
                        continue;
                    }
                };

                // here we handle the merging, which is a bit more complicated
                // than the regular merging due to the Optional type
                let new_pkg_res_opt = api_result.get_mut(&pkg_name).unwrap();
                let mut new_pkg_res = new_pkg_res_opt.take().unwrap();

                for dl in &pkg_res.downloads {
                    new_pkg_res.downloads.push(dl.clone());
                }

                new_pkg_res.end = pkg_res.end.clone();
                *new_pkg_res_opt = Some(new_pkg_res);
            }
            if !not_founds.is_empty() {
                println!("Not found: {}", not_founds);
            }
        };
        self.query_abstraction(pkgs, formatter, merger, lbound, rbound)
            .await
    }
}

impl Default for API {
    fn default() -> Self {
        Self::new(3)
    }
}

fn serde_panic_fn<T>(text: &str, e: serde_json::Error) -> ! {
    println!(
        "failed to deserialize into type: {}",
        std::any::type_name::<T>()
    );
    println!("failed to deserialize: {}", text);
    println!("error: {:?}", e);
    std::process::exit(1);
    // panic!("error deserializing");
}

// impl TypeName {
//     fn type_name() -> &'static str {
//         std::any::type_name::<Self>()
//     }
// }

fn parse_resp<T: for<'a> Deserialize<'a>>(text: String) -> Result<T, ApiError> {
    let json = serde_json::from_str::<serde_json::Value>(&text)
        .unwrap_or_else(|e| serde_panic_fn::<T>(&text, e));

    if let Some(json_map) = json.as_object() {
        if let Some(error) = json_map.get("error").and_then(|e| e.as_str()) {
            if error.contains("not found") {
                return Err(ApiError::DoesNotExist);
            } else {
                return Err(ApiError::Other(error.to_string()));
            }
        }
    }

    Ok(serde_json::from_str(&text).unwrap_or_else(|e| serde_panic_fn::<T>(&text, e)))
}

pub type BulkApiResult = HashMap<String, Option<ApiResult>>;

#[derive(Deserialize, Debug, Clone)]
pub struct ApiResult {
    pub downloads: Vec<ApiResultDownload>,
    pub end: String,
    pub package: String,
    pub start: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ApiResultDownload {
    pub day: String,
    pub downloads: Option<i64>,
}

#[derive(Debug)]
pub enum ApiError {
    Reqwest(reqwest::Error),
    Io(std::io::Error),
    DoesNotExist,
    Other(String), // where String is the error message
    RateLimit,
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        ApiError::Reqwest(err)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::Io(err)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Reqwest(err) => write!(f, "reqwest error: {}", err),
            ApiError::Io(err) => write!(f, "io error: {}", err),
            ApiError::RateLimit => write!(f, "rate-limited"),
            ApiError::DoesNotExist => write!(f, "package does not exist"),
            ApiError::Other(msg) => write!(f, "other error: {}", msg),
        }
    }
}

impl std::error::Error for ApiError {}
