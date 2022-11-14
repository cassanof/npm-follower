use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum BlobError {
    AlreadyExists,
    CreateNotLocked,
    DuplicateKeys,
    DoesNotExist,
    NotWritten,
    WrongNode,
    LockExpired,
    ProhibitedKey,
}

#[derive(Debug)]
pub enum JobError {
    /// The maximum number of worker jobs is already running.
    MaxWorkerJobsReached,
    /// Error related to ssh.
    SshError(openssh::Error),
    /// The command failed.
    CommandFailed { cmd: String, output: String },
    /// The command returned a non-zero exit code.
    CommandNonZero { cmd: String, output: String },
    /// Error from the client of a job.
    ClientError(ClientError),
    /// The output of the client wasn't parsable.
    ClientOutputNotParsable(String),
}

/// Errors that the client can return. This enum is serialized to JSON and sent to the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientError {
    /// Some download urls failed. The vector contains the urls that failed.
    DownloadFailed { urls: Vec<String> },
    /// Some IO error occurred.
    IoError,
    /// Some reqwest error occurred.
    ReqwestError,
    /// Some blob error occurred while create lock a blob.
    BlobCreateLockError,
    /// Some blob error occurred while unlocking a blob.
    BlobUnlockError,
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::AlreadyExists => write!(f, "Blob already exists"),
            BlobError::CreateNotLocked => write!(f, "Blob create not locked"),
            BlobError::DuplicateKeys => write!(f, "Blob duplicate keys"),
            BlobError::DoesNotExist => write!(f, "Blob does not exist"),
            BlobError::ProhibitedKey => write!(f, "Blob key is prohibited"),
            BlobError::WrongNode => write!(f, "Blob is locked by another node"),
            BlobError::LockExpired => write!(f, "Blob lock expired"),
            BlobError::NotWritten => write!(f, "Blob is not written"),
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::MaxWorkerJobsReached => write!(f, "Maximum number of worker jobs reached"),
            JobError::SshError(e) => write!(f, "Ssh error: {}", e),
            JobError::CommandFailed { cmd, output } => {
                write!(f, "Command failed: {} - {}", cmd, output)
            }
            JobError::CommandNonZero { cmd, output } => {
                write!(
                    f,
                    "Command returned non-zero exit code: {} - {}",
                    cmd, output
                )
            }
            JobError::ClientError(e) => write!(f, "Client error: {}", e),
            JobError::ClientOutputNotParsable(s) => {
                write!(f, "Client output not parsable: {}", s)
            }
        }
    }
}

/// Display is implemented here using Serialize, so that the error can be sent to the server.
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap())
    }
}

impl From<openssh::Error> for JobError {
    fn from(e: openssh::Error) -> Self {
        JobError::SshError(e)
    }
}

impl From<std::io::Error> for ClientError {
    fn from(_: std::io::Error) -> Self {
        ClientError::IoError
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(_: reqwest::Error) -> Self {
        ClientError::ReqwestError
    }
}

impl std::error::Error for JobError {}
impl std::error::Error for BlobError {}
impl std::error::Error for ClientError {}
