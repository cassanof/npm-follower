use std::sync::Arc;

use metrics_logging::{JobSchedulerEndSessionMetrics, MetricsLoggerTrait};
use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    debug,
    errors::{ClientError, JobError},
    job::worker::WorkerStatus,
    ssh::SshFactory,
};

use self::pool::WorkerPool;

pub(super) mod pool;
pub(super) mod worker;

/// The response that the worker client sends to the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientResponse {
    Message(serde_json::Value),
    Error(ClientError),
}

/// The result for a single tarball computed by a worker.
#[derive(Debug, Serialize, Deserialize)]
pub struct TarballResult {
    pub exit_code: i32,
    // both of these below are base64 encoded
    pub stdout: String,
    pub stderr: String,
}

/// Configuration to initialize a job manager.
pub struct JobManagerConfig {
    /// The ssh factory to use to create ssh sessions.
    pub ssh_factory: Box<dyn SshFactory>,
    /// The maximum amount of worker jobs that can be running at the same time for compute workers
    pub max_comp_worker_jobs: usize,
    /// The maximum amount of worker jobs that can be running at the same time for transfer workers
    pub max_xfer_worker_jobs: usize,
}

pub struct JobManager {
    xfer_pool: WorkerPool,
    compute_pool: Arc<WorkerPool>,
    start_time: chrono::DateTime<chrono::Utc>,
    metrics_logger: Mutex<metrics_logging::MetricsLogger>,
}

impl JobManager {
    pub async fn init(config: JobManagerConfig) -> Self {
        let arc_ssh_factory = Arc::new(config.ssh_factory);
        debug!(
            "Initializing job manager with {} xfer workers and {} compute workers",
            config.max_xfer_worker_jobs, config.max_comp_worker_jobs
        );
        let mut xfer_pool = WorkerPool::init(
            config.max_xfer_worker_jobs,
            "wp_xfer",
            arc_ssh_factory.clone(),
        )
        .await;
        xfer_pool
            .populate()
            .await
            .expect("populate worker pool failed");

        let mut compute_pool =
            WorkerPool::init(config.max_comp_worker_jobs, "wp_comp", arc_ssh_factory).await;
        compute_pool
            .populate()
            .await
            .expect("populate worker pool failed");

        println!("Job manager initialized");

        let start_time = chrono::Utc::now();
        let mut metrics_logger = metrics_logging::new_metrics_logger(false);
        metrics_logger.log_job_scheduler_start_session(
            metrics_logging::JobSchedulerStartSessionMetrics {
                session_start_time: start_time,
                session_xfer_worker_num: config.max_xfer_worker_jobs,
                session_comp_worker_num: config.max_comp_worker_jobs,
            },
        );

        Self {
            xfer_pool,
            start_time,
            compute_pool: Arc::new(compute_pool),
            metrics_logger: Mutex::new(metrics_logger),
        }
    }

    /// Submits a download and write job to the discovery cluster.
    pub async fn submit_download_job(&self, urls: Vec<String>) -> Result<(), JobError> {
        debug!("Submitting download job with {} urls", urls.len());
        let worker = self.xfer_pool.get_worker().await?;

        let (node_id, ssh) = match &*worker.status {
            WorkerStatus::Running {
                node_id,
                ssh_session,
                ..
            } => (node_id, ssh_session),
            _ => panic!("Worker should be running"),
        };

        let urls = urls.join(" ");

        let cmd = format!(
            "cd $HOME/npm-follower/blob_idx_client && ./run.sh write {} \"{}\"",
            node_id, urls
        );

        debug!("Running command:\n{}", cmd);

        let out = ssh.run_command(&cmd).await?;
        debug!("Output:\n{}", out);

        // parse into a ClientResponse
        let response: ClientResponse =
            serde_json::from_str(&out).map_err(|_| JobError::ClientOutputNotParsable(out))?;

        match response {
            ClientResponse::Message(_) => Ok(()),
            ClientResponse::Error(e) => Err(JobError::ClientError(e)),
        }
    }

    /// Submits a read job to the discovery cluster. Returns the data in base64 format.
    /// This should not be used for computation, just for situational retrieval
    /// of data.
    pub async fn submit_read_job(&self, key: String) -> Result<String, JobError> {
        debug!("Submitting read job with key {}", key);
        let worker = self.xfer_pool.get_worker().await?;
        let ssh = worker.get_ssh_session();

        let cmd = format!(
            "cd $HOME/npm-follower/blob_idx_client && ./run.sh read {}",
            key
        );

        debug!("Running command:\n{}", cmd);

        let out = ssh.run_command(&cmd).await?;
        debug!("Output:\n{}", out);

        // parse into a ClientResponse
        let response: ClientResponse = serde_json::from_str(&out)
            .map_err(|_| JobError::ClientOutputNotParsable(out.clone()))?;

        match response {
            ClientResponse::Message(filepath) => Ok(filepath.as_str().unwrap().to_string()),
            ClientResponse::Error(e) => Err(JobError::ClientError(e)),
        }
    }

    /// Submits a compute job to the discovery cluster. Returns stdout for each tarball computed.
    /// Takes in the full path to the binary to run and a chunk of tarballs, where for each
    /// outer element, we have a list of tarballs to compute on a single node. We map
    /// all chunks to different nodes. We return a list of client responses, where
    /// the index of the response corresponds to the index of the chunk in the list of chunks.
    pub async fn submit_compute(
        &self,
        binary: String,
        tarball_chunks: Vec<Vec<String>>,
        timeout: u64,
    ) -> Result<Vec<ClientResponse>, JobError> {
        let mut handles: Vec<JoinHandle<Result<ClientResponse, JobError>>> = Vec::new();

        for chunk in &tarball_chunks {
            debug!("Submitting compute job with {} tarballs", chunk.len());
            let wp_comp = self.compute_pool.clone();
            let binary = binary.clone();
            let tbs = chunk.join(" ");
            handles.push(tokio::task::spawn(async move {
                let worker = wp_comp.get_worker().await?;
                let ssh = worker.get_ssh_session();
                let cmd = format!(
                    "cd $HOME/npm-follower/blob_idx_client && ./run.sh compute {} \"{}\"",
                    binary, tbs
                );
                debug!("Running command:\n{}", cmd);
                let out = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    ssh.run_command(&cmd),
                )
                .await
                {
                    Ok(res) => res?,
                    Err(_) => {
                        wp_comp.replace_worker(&worker).await?;
                        return Ok(ClientResponse::Error(ClientError::Timeout));
                    }
                };
                debug!("Output:\n{}", out);
                let response: ClientResponse = serde_json::from_str(&out)
                    .map_err(|_| JobError::ClientOutputNotParsable(out))?;
                Ok(response)
            }));
        }

        let mut responses = Vec::new();
        for handle in handles {
            responses.push(handle.await.unwrap()?);
        }

        Ok(responses)
    }

    /// Submits a compute job to the discovery cluster. Returns stdout for each tarball computed.
    /// Takes in the full path to the binary to run and a chunk of tarballs, where for each
    /// outer element, we have a list of tarballs to compute on a single node. We map
    /// all chunks to different nodes. We return a list of client responses, where
    /// the index of the response corresponds to the index of the chunk in the list of chunks.
    /// This is a multi-arg version of the above function.
    pub async fn submit_compute_multi(
        &self,
        binary: String,
        tarball_chunks: Vec<Vec<Vec<String>>>,
        timeout: u64,
    ) -> Result<Vec<ClientResponse>, JobError> {
        let mut handles: Vec<JoinHandle<Result<ClientResponse, JobError>>> = Vec::new();

        for chunk in &tarball_chunks {
            debug!("Submitting compute job with {} tarballs", chunk.len());
            let wp_comp = self.compute_pool.clone();
            let binary = binary.clone();
            let tbs = chunk
                .iter()
                .map(|sub| sub.join("&"))
                .collect::<Vec<String>>()
                .join(" ");
            handles.push(tokio::task::spawn(async move {
                let worker = wp_comp.get_worker().await?;
                let ssh = worker.get_ssh_session();
                let cmd = format!(
                    "cd $HOME/npm-follower/blob_idx_client && ./run.sh compute_multi {} \"{}\"",
                    binary, tbs
                );
                debug!("Running command:\n{}", cmd);

                let out = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    ssh.run_command(&cmd),
                )
                .await
                {
                    Ok(res) => res?,
                    Err(_) => {
                        println!("Worker timed out! Replacing...");
                        wp_comp.replace_worker(&worker).await?;
                        return Ok(ClientResponse::Error(ClientError::Timeout));
                    }
                };

                debug!("Output:\n{}", out);
                let response: ClientResponse = serde_json::from_str(&out)
                    .map_err(|_| JobError::ClientOutputNotParsable(out))?;
                Ok(response)
            }));
        }

        let mut responses = Vec::new();
        for handle in handles {
            responses.push(handle.await.unwrap()?);
        }

        Ok(responses)
    }

    /// Stores the files in the given filepaths (that reside on the discovery cluster) into
    /// the blob index. The filepaths should be the full path to the file on the discovery cluster.
    pub async fn submit_store_tarballs(&self, filepaths: Vec<String>) -> Result<(), JobError> {
        debug!(
            "Submitting store tarballs job with {} filepaths",
            filepaths.len()
        );
        let worker = self.xfer_pool.get_worker().await?;
        let (node_id, ssh) = match &*worker.status {
            WorkerStatus::Running {
                node_id,
                ssh_session,
                ..
            } => (node_id, ssh_session),
            _ => panic!("Worker should be running"),
        };

        let filepaths = filepaths.join(" ");

        let cmd = format!(
            "cd $HOME/npm-follower/blob_idx_client && ./run.sh store {} \"{}\"",
            node_id, filepaths
        );

        debug!("Running command:\n{}", cmd);

        let out = ssh.run_command(&cmd).await?;
        debug!("Output:\n{}", out);

        // parse into a ClientResponse
        let response: ClientResponse =
            serde_json::from_str(&out).map_err(|_| JobError::ClientOutputNotParsable(out))?;

        match response {
            ClientResponse::Message(_) => Ok(()),
            ClientResponse::Error(e) => Err(JobError::ClientError(e)),
        }
    }

    /// Shuts down the client. Sends a message to the logger and cancels all jobs in the pools.
    pub async fn shutdown(&self) {
        let end_time = chrono::Utc::now();
        self.metrics_logger
            .lock()
            .await
            .log_job_scheduler_end_session(JobSchedulerEndSessionMetrics {
                session_start_time: self.start_time,
                session_end_time: end_time,
                session_total_duration: end_time - self.start_time,
            });

        self.xfer_pool.shutdown().await;
        self.compute_pool.shutdown().await;
    }
}
