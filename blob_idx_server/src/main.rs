use blob_idx_server::{
    blob,
    http::HTTP,
    job::JobManagerConfig,
    ssh::{SshSessionFactory},
};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let api_key = std::env::var("BLOB_API_KEY").expect("API_KEY must be set");
    let http = HTTP::new("127.0.0.1".to_string(), "8080".to_string(), api_key);
    let discovery_ssh = std::env::var("DISCOVERY_SSH").expect("DISCOVERY_SSH must be set");
    let (_tx, mut shutdown_signal) = tokio::sync::mpsc::channel::<()>(1);

    http.start(
        blob::BlobStorageConfig::default(),
        JobManagerConfig {
            ssh_factory: Box::new(SshSessionFactory::new(&discovery_ssh)),
            max_worker_jobs: 10,
        },
        async move {
            shutdown_signal.recv().await;
        },
    )
    .await
    .expect("Failed to start http server");
}
