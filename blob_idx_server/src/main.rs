use blob_idx_server::{blob, http::HTTP, job::JobManagerConfig, ssh::SshSessionFactory};

/// Awaits a shutdown signal from the OS, e.g. Ctrl-C
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("expect tokio signal ctrl-c");
    println!("Signal shutdown initiated");
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let api_key = std::env::var("BLOB_API_KEY").expect("API_KEY must be set");
    let http = HTTP::new("127.0.0.1".to_string(), "8080".to_string(), api_key);
    let discovery_ssh = std::env::var("DISCOVERY_SSH").expect("DISCOVERY_SSH must be set");

    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <num compute workers> <num xfer workers>",
            args[0]
        );
        std::process::exit(1);
    }

    let service_state = http
        .start(
            blob::BlobStorageConfig::default(),
            JobManagerConfig {
                ssh_factory: Box::new(SshSessionFactory::new(&discovery_ssh)),
                max_comp_worker_jobs: args[1].parse().unwrap(),
                max_xfer_worker_jobs: args[2].parse().unwrap(),
            },
            shutdown_signal(),
        )
        .await
        .expect("Failed to start http server");

    println!("Shutting down!");

    service_state.job_manager.shutdown().await;
    service_state.blob.shutdown().await;
}
