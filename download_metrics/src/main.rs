use std::collections::HashMap;
use std::sync::Arc;

use chrono::NaiveDate;
use download_metrics::api::query_npm_metrics;
use download_metrics::api::ApiError;
use download_metrics::LOWER_BOUND_DATE;
use download_metrics::UPPER_BOUND_DATE;
use postgres_db::download_metrics::QueriedDownloadMetric;
use postgres_db::{
    custom_types::DownloadCount, download_metrics::DownloadMetric, packages::QueriedPackage,
    DbConnection,
};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use utils::check_no_concurrent_processes;

#[tokio::main]
async fn main() {
    check_no_concurrent_processes("download_metrics");
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        eprintln!("Usage: {} <insert|update>", args[0]);
        std::process::exit(1);
    }
    let conn = postgres_db::connect();

    match args[1].as_str() {
        "insert" => insert_from_packages(&conn).await,
        "update" => update_from_packages(&conn).await,
        _ => {
            eprintln!("Usage: {} <insert|update>", args[0]);
            std::process::exit(1);
        }
    }
}

async fn make_download_metric(
    pkg: &QueriedPackage,
    sem: Arc<Semaphore>,
    lower_bound_date: &NaiveDate,
    upper_bound_date: &NaiveDate,
) -> Result<DownloadMetric, ApiError> {
    let permit = sem.acquire().await.unwrap();
    let result = query_npm_metrics(pkg, lower_bound_date, upper_bound_date).await?;
    drop(permit);

    // we need to convert the results into DownloadMetric, merging daily results
    // into weekly results
    let mut weekly_results: Vec<DownloadCount> = Vec::new();
    let mut i = 0;
    let mut total_downloads = 0;

    loop {
        let mut weekly_count = result.downloads[i].downloads;
        let mut j = i + 1;
        while j < result.downloads.len() && j < i + 7 {
            weekly_count += result.downloads[j].downloads;
            j += 1;
        }

        let date = chrono::NaiveDate::parse_from_str(&result.downloads[i].day, "%Y-%m-%d").unwrap();

        // we set i to j so that we skip the days we already counted
        i = j;

        total_downloads += weekly_count;

        let count = DownloadCount {
            date,
            count: weekly_count,
        };

        // we don't insert zero counts
        if weekly_count > 0 {
            weekly_results.push(count);
        }

        if i >= result.downloads.len() {
            // we still want to know the latest, even if it's zero and we didn't insert it
            let latest = date;
            println!("did package {}", pkg.name);
            return Ok(DownloadMetric::new(
                pkg.id,
                weekly_results,
                total_downloads,
                latest,
            ));
        }
    }
}

async fn update_from_packages(conn: &DbConnection) {
    let mut metrics = query_metrics_older_than_a_week(conn);

    while !metrics.is_empty() {
        let mut handles: Vec<JoinHandle<Result<(i64, DownloadMetric), ApiError>>> = Vec::new();
        let sem = Arc::new(Semaphore::new(3));

        // map of [metric id] -> [metric]
        let mut lookup_table: HashMap<i64, DownloadMetric> = HashMap::new();

        for metric in metrics {
            let pkg = postgres_db::packages::query_pkg_by_id(conn, metric.package_id)
                .expect("coulnd't find package from metric's package_id");

            let lower_bound_date = metric.latest_date + chrono::Duration::days(1);

            let metric_id = metric.id;
            lookup_table.insert(metric_id, metric.into());

            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                make_download_metric(&pkg, sem, &lower_bound_date, &UPPER_BOUND_DATE)
                    .await
                    .map(|m| (metric_id, m))
            }));
        }

        // where i64 is the id of the metric
        let mut metrics_to_upd: Vec<(i64, DownloadMetric)> = Vec::new();

        for handle in handles {
            let (id, mut metric) = match handle.await.unwrap() {
                Ok(metric) => metric,
                Err(ApiError::RateLimit) => {
                    eprintln!("Rate limited! Exiting!");
                    std::process::exit(1);
                }
                Err(e) => {
                    println!("Error: {}", e);
                    continue;
                }
            };

            println!("got update metric for {}", metric.package_id);
            println!("new latest date: {}", metric.latest_date);
            println!("new counts:");
            for count in &metric.download_counts {
                print!("{}: {}, ", count.date, count.count);
            }
            println!();

            let older_metric = lookup_table.get(&id).unwrap();
            metric.download_counts = older_metric
                .download_counts
                .iter()
                .chain(metric.download_counts.iter())
                .cloned()
                .collect();
            metric.total_downloads += older_metric.total_downloads;
            metrics_to_upd.push((id, metric));
        }

        conn.run_psql_transaction(|| {
            for (id, metric) in metrics_to_upd {
                postgres_db::download_metrics::update_metric_by_id(conn, id, metric);
            }
            Ok(())
        })
        .expect("couldn't run transaction");

        metrics = query_metrics_older_than_a_week(conn);
    }

    println!("Done updating metrics");
}

/// Queries all metrics older than a week. The query is limited to 1000 results.
fn query_metrics_older_than_a_week(conn: &DbConnection) -> Vec<QueriedDownloadMetric> {
    let week_ago = get_a_week_ago(&LOWER_BOUND_DATE, &UPPER_BOUND_DATE) - chrono::Duration::days(7);
    println!("querying metrics older than {}", week_ago);
    postgres_db::download_metrics::query_metric_latest_less_than(conn, week_ago)
}

/// Inserts new download metric rows by using the `packages` table and querying npm
async fn insert_from_packages(conn: &DbConnection) {
    let mut pkg_id = postgres_db::internal_state::query_download_metrics_pkg_seq(conn).unwrap_or(1);

    println!("starting inserting metrics from pkg_id: {}", pkg_id);

    // NOTE: Bulk queries are limited to at most 128 packages at a time and at most 365 days of data.
    //       however, we can't bulk query scoped packages.

    // therefore we run in chunks of 128 packages (+ scoped packages, max 128 too for consistency)

    let mut finished = false; // we break the loop if we have no more packages to query
    while !finished {
        let mut chunk_pkg_id = pkg_id;
        let mut normal_packages = Vec::new();
        let mut scoped_packages = Vec::new(); // TODO: do scoped packages.

        while normal_packages.len() < 128 && scoped_packages.len() < 128 {
            let pkg = postgres_db::packages::query_pkg_by_id(conn, chunk_pkg_id);
            match pkg {
                None => {
                    // could be that ids are not contiguous, so we need to get the next id
                    let next_pkg_id = postgres_db::packages::query_next_pkg_id(conn, chunk_pkg_id);
                    match next_pkg_id {
                        None => {
                            // no more packages to query
                            finished = true;
                            break;
                        }
                        Some(next_pkg_id) => {
                            println!(
                                "No package with id {}, skipping to next id {}",
                                chunk_pkg_id, next_pkg_id
                            );
                            chunk_pkg_id = next_pkg_id;
                        }
                    }
                }
                Some(pkg) => {
                    // TODO: i think? ping donald about it
                    if !pkg.secret && has_normal_metadata(&pkg) {
                        if pkg.name.starts_with('@') {
                            scoped_packages.push(pkg);
                        } else {
                            normal_packages.push(pkg);
                        }
                    }
                    chunk_pkg_id += 1;
                }
            }
        }

        let mut download_metrics: Vec<DownloadMetric> = Vec::new();
        let mut handles: Vec<JoinHandle<Result<DownloadMetric, ApiError>>> = Vec::new();
        let sem = Arc::new(Semaphore::new(3)); // limiting to 3 requests at a time

        // TODO: bulk query, remove chain
        for pkg in normal_packages
            .into_iter()
            .chain(scoped_packages.into_iter())
        {
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                make_download_metric(&pkg, sem, &LOWER_BOUND_DATE, &UPPER_BOUND_DATE).await
            }));
        }

        for handle in handles {
            let metric = match handle.await.unwrap() {
                Ok(metric) => metric,
                Err(ApiError::RateLimit) => {
                    eprintln!("Rate limited! Exiting!");
                    std::process::exit(1);
                }
                Err(e) => {
                    println!("Error: {}", e);
                    continue;
                }
            };

            println!("latest: {:?}", metric.latest_date);
            println!("counts:");
            for dl in &metric.download_counts {
                print!("{}: {}, ", dl.date, dl.count);
            }
            println!();
            download_metrics.push(metric);
        }

        conn.run_psql_transaction(|| {
            for metric in download_metrics {
                postgres_db::download_metrics::insert_download_metric(conn, metric);
            }
            postgres_db::internal_state::set_download_metrics_pkg_seq(chunk_pkg_id, conn);
            Ok(())
        })
        .expect("failed to insert download metrics");
        pkg_id = chunk_pkg_id;
    }

    println!("Done, at pkg_id {}", pkg_id);
}

/// Helper to check if a package has normal metadata
fn has_normal_metadata(pkg: &QueriedPackage) -> bool {
    use postgres_db::custom_types::PackageMetadata;
    matches!(
        pkg.metadata,
        PackageMetadata::Normal {
            dist_tag_latest_version: _,
            created: _,
            modified: _,
            other_dist_tags: _,
        }
    )
}

/// Returns the earliest date that matches a week given the epoch, using the same logic as npm
/// queries.
fn get_a_week_ago(lbound: &chrono::NaiveDate, rbound: &chrono::NaiveDate) -> NaiveDate {
    let delta = chronoutil::RelativeDuration::years(1);
    let mut rel_lbound = *lbound;
    let mut res = *lbound;
    let rule = chronoutil::DateRule::new(rel_lbound + delta, delta);
    for mut rel_rbound in rule {
        if rel_lbound > *rbound {
            break;
        }

        if rel_rbound > *rbound {
            rel_rbound = *rbound;
        }

        res = rel_rbound;
        rel_lbound = rel_rbound + chronoutil::RelativeDuration::days(1);
    }

    // now traverse weeks, until we get a week less than `res`
    let delta = chrono::Duration::weeks(1);
    let mut rel_lbound = *lbound;
    let rbound = res;

    while rel_lbound < rbound {
        rel_lbound += delta;
    }

    rel_lbound - delta
}
