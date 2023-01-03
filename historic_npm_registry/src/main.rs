use std::str::FromStr;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use moka::future::Cache;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde_json::Map;
use serde_json::Value;
use warp::http::StatusCode;
use warp::reply;
use warp::Filter;
use warp::Rejection;
use warp::Reply;

type NpmCache = Cache<String, Arc<Value>>;

fn restrict_time(v: &Value, t: DateTime<Utc>) -> Value {
    v.clone()
}

async fn request_package_from_npm(full_name: &str, client: ClientWithMiddleware) -> Value {
    println!("hitting NPM for: {}", full_name);

    let packument_doc = client
        .get(format!("https://registry.npmjs.org/{}", full_name))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    let packument_doc = match packument_doc {
        Value::Object(o) => o,
        _ => panic!("non-object packument"),
    };

    let packument = diff_log_builder::deserialize_packument_doc(packument_doc, None, None);
    println!("parsed: {:?}", packument);
    Value::Null
}

async fn lookup_package(
    t: DateTime<Utc>,
    full_name: String,
    client: ClientWithMiddleware,
    cache: NpmCache,
) -> Value {
    println!("looking up: {}", full_name);
    if let Some(cache_hit) = cache.get(&full_name) {
        restrict_time(&cache_hit, t)
    } else {
        let npm_response = request_package_from_npm(&full_name, client).await;

        let npm_response = Arc::new(npm_response);
        cache.insert(full_name, npm_response.clone()).await;
        restrict_time(&npm_response, t)
    }
}

async fn handle_request(
    t_str_url_encoded: String,
    scope: Option<String>,
    name: String,
    client: ClientWithMiddleware,
    cache: NpmCache,
) -> warp::reply::Json {
    println!("handle_request");

    let full_name = if let Some(s) = scope {
        format!("{}/{}", s, name)
    } else {
        name
    };

    let t_str = percent_encoding::percent_decode(t_str_url_encoded.as_bytes())
        .decode_utf8()
        .unwrap();

    if let Ok(t) = DateTime::<Utc>::from_str(&t_str) {
        warp::reply::json(&lookup_package(t, full_name, client, cache).await)
    } else {
        panic!("BAD DATE: {}", t_str)
    }
}

// Custom rejection handler that maps rejections into responses.
async fn handle_rejection(err: Rejection) -> Result<impl Reply, std::convert::Infallible> {
    eprintln!("unhandled rejection: {:?}", err);
    Ok(reply::with_status(
        "INTERNAL_SERVER_ERROR",
        StatusCode::INTERNAL_SERVER_ERROR,
    ))
}

#[tokio::main]
async fn main() {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(6);
    let req_client = ClientBuilder::new(reqwest::Client::new())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();
    let req_client2 = req_client.clone();

    let cache = Cache::new(4_194_304);
    let cache2 = cache.clone();

    let non_scoped = warp::path::param::<String>()
        .and(warp::path::param::<String>())
        .and(warp::any().map(move || req_client.clone()))
        .and(warp::any().map(move || cache.clone()))
        .then(
            |t_str_url: String, name, req_client_inner, cache| async move {
                handle_request(t_str_url, None, name, req_client_inner, cache).await
            },
        );

    let scoped = warp::path::param::<String>()
        .and(warp::path::param::<String>())
        .and(warp::path::param::<String>())
        .and(warp::any().map(move || req_client2.clone()))
        .and(warp::any().map(move || cache2.clone()))
        .then(
            |t_str_url: String, scope, name, req_client_inner, cache| async move {
                handle_request(t_str_url, Some(scope), name, req_client_inner, cache).await
            },
        );

    let empty_advisories =
        warp::path!(String / "-" / "npm" / "v1" / "security" / "advisories" / "bulk")
            .map(|_t| warp::reply::json(&Value::Object(Map::default())));

    warp::serve(
        empty_advisories
            .or(non_scoped)
            .or(scoped)
            .recover(handle_rejection),
    )
    .run(([0, 0, 0, 0], 80))
    .await;
}
