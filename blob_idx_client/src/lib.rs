use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use blob_idx_server::{
    blob::{BlobOffset, BlobStorageSlice},
    errors::ClientError,
    http::{
        BlobEntry, CreateAndLockRequest, CreateUnlockRequest, KeepAliveLockRequest, LookupRequest,
    },
    job::TarballResult,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Semaphore,
    task::JoinHandle,
};

fn spawn_keep_alive_loop(file_id: u32) -> JoinHandle<()> {
    tokio::task::spawn(async move {
        let blob_api_url = std::env::var("BLOB_API_URL").expect("BLOB_API_URL must be set");
        let blob_api_key = std::env::var("BLOB_API_KEY").expect("BLOB_API_KEY must be set");
        let req = KeepAliveLockRequest { file_id };
        let client = reqwest::Client::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            eprintln!("Sending keep-alive request");

            let res = client
                .post(format!("{}/blob/keep_alive_lock", blob_api_url))
                .header("Authorization", blob_api_key.clone())
                .json(&req)
                .send()
                .await;

            if res.is_err() {
                break;
            }
        }
    })
}

fn make_client() -> Result<reqwest::Client, ClientError> {
    Ok(reqwest::ClientBuilder::new()
        .connect_timeout(std::time::Duration::from_secs(60))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent("Wget/1.21.3")
        .build()?)
}

/// Checks if the response was successful, and if not, returns an error.
async fn check_req_failed(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.map_err(|e| {
            ClientError::ReqwestError(format!("Failed to read response text: {}", e))
        })?;
        // try to parse into a BlobError, or send HttpServerError
        let mut err_map = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
            .map_err(|_| ClientError::SerdeJsonError(format!("Failed to parse error: {}", text)))?;
        eprintln!(
            "Failed request. Got: {}",
            err_map.get("error").unwrap_or(&serde_json::Value::Null)
        );
        if let Some(err) = err_map.remove("error") {
            let blob_err = serde_json::from_value::<blob_idx_server::errors::BlobError>(err)?;
            Err(ClientError::BlobError(blob_err))
        } else {
            Err(ClientError::HttpServerError(
                std::num::NonZeroU16::new(status.as_u16()).unwrap(),
            ))
        }
    } else {
        Ok(resp)
    }
}

async fn read_slice(tarball_url_key: String) -> Result<BlobStorageSlice, ClientError> {
    let blob_api_url = std::env::var("BLOB_API_URL").expect("BLOB_API_URL must be set");
    let blob_api_key = std::env::var("BLOB_API_KEY").expect("BLOB_API_KEY must be set");

    let client = make_client()
        .map_err(|e| ClientError::ReqwestError(format!("Failed to build client: {}", e)))?;

    // lookup request
    eprintln!("Sending lookup request for {}", tarball_url_key);
    let body = serde_json::to_vec(&LookupRequest {
        key: tarball_url_key,
    })?;
    let resp = client
        .get(format!("{}/blob/lookup", blob_api_url))
        .header("Authorization", blob_api_key.clone())
        .body(body)
        .send()
        .await
        .map_err(|e| ClientError::ReqwestError(format!("Failed to send request: {}", e)))?;

    let resp = check_req_failed(resp).await?;

    let text = resp
        .text()
        .await
        .map_err(|e| ClientError::ReqwestError(format!("Failed to read response body: {}", e)))?;
    let slice: BlobStorageSlice = serde_json::from_str(&text)
        .map_err(|e| ClientError::SerdeJsonError(format!("{} - {}", text, e)))?;
    Ok(slice)
}

pub async fn compute_run_bin(
    args: Vec<String>,
) -> Result<HashMap<String, TarballResult>, ClientError> {
    if args.len() != 4 {
        eprintln!(
            "Usage: {} compute <binary path> <tarball keys, separated by spaces>",
            args[0]
        );
        std::process::exit(1);
    }

    let binary = args[2].clone();
    let tarball_url_keys = args[3].clone();
    let tarball_url_keys: Vec<String> =
        tarball_url_keys.split(' ').map(|s| s.to_string()).collect();

    let mut handles: Vec<JoinHandle<Result<(String, String), ClientError>>> = Vec::new();
    let mut slice_map = HashMap::new(); // map of [tmp slice path] -> [original tarball url]
    let thunk = async {
        let pid = std::process::id();
        let atomic_idx = Arc::new(AtomicUsize::new(0));
        for tarball_url_key in tarball_url_keys.clone() {
            let atomic_idx = atomic_idx.clone();
            let handle = tokio::task::spawn(async move {
                let slice_path = read_and_send(
                    tarball_url_key.clone(),
                    &format!(
                        "/tmp/compute-{}/{}",
                        pid,
                        atomic_idx.fetch_add(1, Ordering::SeqCst)
                    ),
                )
                .await?;
                Ok((tarball_url_key, slice_path))
            });
            handles.push(handle);
        }

        for handle in handles {
            let (tarball_url, slice_path) = handle.await.unwrap()?;
            slice_map.insert(slice_path, tarball_url);
        }

        // check that the binary exists
        if !std::path::Path::new(&binary).exists() {
            return Err(ClientError::BinaryDoesNotExist);
        }

        let mut handle_map: HashMap<String, JoinHandle<Result<TarballResult, ClientError>>> =
            HashMap::new(); // where the string is the original tarball url
        for (slice_path, original_tarball_url) in slice_map.iter() {
            let mut cmd = tokio::process::Command::new(&binary);
            cmd.arg(slice_path);
            // slice_paths.push(slice_path);
            let handle = tokio::task::spawn(async move {
                let output = cmd.output().await?;
                // make sure the base64 does not put newlines
                let b64stout = base64::encode_config(&output.stdout, base64::STANDARD_NO_PAD);
                let b64stderr = base64::encode_config(&output.stderr, base64::STANDARD_NO_PAD);
                let exit_code = output.status.code().unwrap_or(1);
                Ok(TarballResult {
                    stdout: b64stout,
                    stderr: b64stderr,
                    exit_code,
                })
            });
            handle_map.insert(original_tarball_url.to_string(), handle);
        }

        let mut res_map = HashMap::new();
        for (original_tarball_url, handle) in handle_map {
            let res = handle.await.unwrap()?;
            res_map.insert(original_tarball_url, res);
        }
        Ok(res_map)
    };

    let res = thunk.await;

    // now delete the tmp file directories
    if let Some(slice_path) = slice_map.keys().next() {
        // go up three directories
        let p = std::path::Path::new(&slice_path)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        // first, we have to recursively set the permissions to be writable
        let mut cmd = tokio::process::Command::new("chmod");
        cmd.arg("-R")
            .arg("777")
            .arg(p.to_str().unwrap())
            .output()
            .await
            .ok();
        eprintln!("Deleting {}", p.to_str().unwrap());
        tokio::fs::remove_dir_all(p).await.ok();
    }

    res
}

pub async fn compute_run_bin_multi(
    args: Vec<String>,
) -> Result<HashMap<String, TarballResult>, ClientError> {
    if args.len() != 4 {
        eprintln!(
            "Usage: {} compute <binary path> <tarball keys, separated by spaces>",
            args[0]
        );
        std::process::exit(1);
    }

    let binary = args[2].clone();
    let tarball_url_keys = args[3].clone();
    let tarball_url_keys: Vec<Vec<String>> = tarball_url_keys
        .split(' ')
        .map(|s| s.split('&').map(|s| s.to_string()).collect())
        .collect();

    let mut handles = Vec::new();
    // map of [Vec<tmp slice path>] -> [Vec<original tarball url>]
    let mut slice_map = HashMap::new();
    let thunk = async {
        let pid = std::process::id();
        let atomic_idx = Arc::new(AtomicUsize::new(0));
        for tarball_url_keys in tarball_url_keys.clone() {
            let atomic_idx = atomic_idx.clone();
            let handle = tokio::task::spawn(async move {
                let mut slice_paths = Vec::new();
                let mut original_tarball_urls = Vec::new();
                for tarball_url_key in tarball_url_keys {
                    let slice_path = read_and_send(
                        tarball_url_key.clone(),
                        &format!(
                            "/tmp/compute-{}/{}",
                            pid,
                            atomic_idx.fetch_add(1, Ordering::SeqCst)
                        ),
                    )
                    .await?;
                    slice_paths.push(slice_path);
                    original_tarball_urls.push(tarball_url_key);
                }
                Ok::<_, ClientError>((slice_paths, original_tarball_urls))
            });
            handles.push(handle);
        }

        eprintln!("Waiting for all slices to be read and sent");

        for handle in handles {
            let (slice_paths, tarball_urls) = handle.await.unwrap()?;
            slice_map.insert(slice_paths, tarball_urls);
        }

        eprintln!("All slices read and sent");

        // check that the binary exists
        if !std::path::Path::new(&binary).exists() {
            return Err(ClientError::BinaryDoesNotExist);
        }

        let mut handle_map: HashMap<String, JoinHandle<Result<TarballResult, ClientError>>> =
            HashMap::new(); // where the string is the original tarball url
        for (slice_paths, original_tarball_urls) in slice_map.iter() {
            let mut cmd = tokio::process::Command::new(&binary);
            cmd.args(slice_paths);
            // slice_paths.push(slice_path);
            let handle = tokio::task::spawn(async move {
                let output = cmd.output().await?;
                // make sure the base64 does not put newlines
                let b64stout = base64::encode_config(&output.stdout, base64::STANDARD_NO_PAD);
                let b64stderr = base64::encode_config(&output.stderr, base64::STANDARD_NO_PAD);
                let exit_code = output.status.code().unwrap_or(1);
                Ok(TarballResult {
                    stdout: b64stout,
                    stderr: b64stderr,
                    exit_code,
                })
            });
            handle_map.insert(original_tarball_urls.join("&").to_string(), handle);
        }

        let mut res_map = HashMap::new();
        for (original_tarball_url, handle) in handle_map {
            let res = handle.await.unwrap()?;
            eprintln!("Got result for {}", original_tarball_url);
            res_map.insert(original_tarball_url, res);
        }
        Ok(res_map)
    };

    let res = thunk.await;

    // now delete the tmp files
    if let Some(slice_path) = slice_map.keys().next().and_then(|s| s.get(0)) {
        // go up three directories
        let p = std::path::Path::new(&slice_path)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        // first, we have to recursively set the permissions to be writable
        let mut cmd = tokio::process::Command::new("chmod");
        cmd.arg("-R")
            .arg("777")
            .arg(p.to_str().unwrap())
            .output()
            .await
            .ok();
        eprintln!("Deleting {}", p.to_str().unwrap());
        tokio::fs::remove_dir_all(p).await.ok();
    }

    res
}

async fn read_and_send(tarball_key: String, tmp_dir_root: &str) -> Result<String, ClientError> {
    let blob_storage_dir = std::env::var("BLOB_STORAGE_DIR").expect("BLOB_STORAGE_DIR must be set");
    let slice = read_slice(tarball_key.to_string()).await?;

    let mut file =
        tokio::fs::File::open(format!("{}/{}", blob_storage_dir, slice.file_name)).await?;
    // seek to the offset
    file.seek(std::io::SeekFrom::Start(slice.byte_offset))
        .await?;

    // read slice.num_bytes from file. make into base64.
    let mut buf = vec![0; slice.num_bytes as usize];
    file.read_exact(&mut buf).await?;

    // write to temp file, the dir is "{tmp_dir_root}/blob_slices/"
    // it may need to be created
    let temp_dir = format!("{}/blob_slices", tmp_dir_root);
    let temp_dir_path = std::path::Path::new(&temp_dir);
    if !temp_dir_path.exists() {
        std::fs::create_dir_all(temp_dir_path)?;
    }
    let temp_file_path = {
        // get pid of process, use that as a unique identifier
        let pid = std::process::id();
        let slurm_job_id =
            std::env::var("SLURM_JOB_ID").unwrap_or_else(|_| slice.file_id.to_string());

        temp_dir_path.join(format!("blob-file-{}-{}", pid, slurm_job_id))
    };

    eprintln!("Writing to temp file: {}", temp_file_path.display());

    // write to temp file
    let mut file = tokio::fs::File::create(&temp_file_path).await?;
    file.write_all(&buf).await?;

    Ok(temp_file_path.to_str().unwrap().to_string())
}

pub async fn read_and_send_main(args: Vec<String>) -> Result<String, ClientError> {
    if args.len() != 3 {
        eprintln!("Usage: {} read <tarball url key>", args[0]);
        std::process::exit(1);
    }

    let tarball_url_key = &args[2];
    let tmp_dir_root = format!("/scratch/{}", std::env::var("USER").unwrap());
    read_and_send(tarball_url_key.to_string(), &tmp_dir_root).await
}

pub async fn cp_main(args: Vec<String>) -> Result<(), ClientError> {
    if args.len() != 4 {
        eprintln!("Usage: {} cp <tarball url key> <destination path>", args[0]);
        std::process::exit(1);
    }

    let dst_path = &args[3];
    let tmp_path = read_and_send_main(args[0..3].to_vec()).await?;

    // Now move the file to the destination path
    match tokio::fs::rename(&tmp_path, dst_path).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.kind().to_string() == "cross-device link or rename" {
                tokio::fs::copy(&tmp_path, dst_path).await?;
                tokio::fs::remove_file(tmp_path).await?;
                Ok(())
            } else {
                Err(e.into())
            }
        }
    }
}

pub async fn download_and_write(args: Vec<String>) -> Result<(), ClientError> {
    if args.len() != 4 {
        eprintln!(
            "Usage: {} write <discovery node id> <tarball urls, separated by spaces>",
            args[0]
        );
        std::process::exit(1);
    }

    let node_id: String = args[2].clone();
    let urls: Vec<String> = args[3].split(' ').map(|s| s.to_string()).collect();

    // download all tarballs
    let sem = Arc::new(Semaphore::new(10)); // max 10 concurrent downloads

    // the join handles will hold the name of the file and it's contents
    // the result, if failed, will return the url that failed
    let mut handles = vec![];
    let client = make_client()?;

    for url in urls {
        let sem = Arc::clone(&sem);
        let url = url.clone();
        let client = client.clone();
        handles.push(tokio::task::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            eprintln!("Downloading {}", url);
            let mut resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(_) => return Err((url, std::num::NonZeroU16::new(1337).unwrap())),
            };
            drop(_permit);
            // check if the response is not an error
            if !resp.status().is_success() {
                let non_zero_status = std::num::NonZeroU16::new(resp.status().as_u16()).unwrap();
                return Err((url, non_zero_status));
            }

            let mut bytes = {
                // if we know the size of the response, we can pre-allocate the buffer, otherwise
                // we'll just use the default Vec::new()
                if let Some(size) = resp.content_length() {
                    Vec::with_capacity(size as usize)
                } else {
                    Vec::new()
                }
            };
            while let Some(chunk) = match resp.chunk().await {
                Ok(c) => c,
                // presumably an io error
                Err(_) => return Err((url, std::num::NonZeroU16::new(500).unwrap())),
            } {
                bytes.extend_from_slice(&chunk);
            }

            Ok((url, bytes))
        }));
    }

    let mut failures = vec![];
    let mut blob_entries = vec![];
    let mut blob_bytes = vec![];
    for handle in handles {
        match handle.await.unwrap() {
            Ok((url, bytes)) => {
                let blob_entry = BlobEntry {
                    key: url.clone(),
                    num_bytes: bytes.len() as u64,
                };
                blob_entries.push(blob_entry);
                blob_bytes.push(bytes);
            }
            Err(url) => {
                failures.push(url);
            }
        }
    }

    // if we have 0 successes, we can't continue
    if blob_bytes.is_empty() {
        return Err(ClientError::DownloadFailed { urls: failures });
    }

    store_into_blob(blob_entries, blob_bytes, node_id).await?;

    if !failures.is_empty() {
        return Err(ClientError::DownloadFailed { urls: failures });
    }
    Ok(())
}

async fn store_into_blob(
    blob_entries: Vec<BlobEntry>,
    blob_bytes: Vec<Vec<u8>>,
    node_id: String,
) -> Result<(), ClientError> {
    let blob_api_url = std::env::var("BLOB_API_URL").expect("BLOB_API_URL must be set");
    let blob_api_key = std::env::var("BLOB_API_KEY").expect("BLOB_API_KEY must be set");
    let blob_storage_dir = std::env::var("BLOB_STORAGE_DIR").expect("BLOB_STORAGE_DIR must be set");

    let entries_keys = blob_entries
        .iter()
        .map(|e| e.key.clone())
        .collect::<Vec<_>>()
        .join(" ");

    // ask the blob api to lock
    let req = CreateAndLockRequest {
        entries: blob_entries,
        node_id: node_id.clone(),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/blob/create_and_lock", blob_api_url))
        .header("Authorization", blob_api_key.clone())
        .json(&req)
        .send()
        .await?;

    // if we get a 200, we can continue
    let resp = check_req_failed(resp).await?;

    // parse the response
    let blob: BlobOffset = resp
        .json()
        .await
        .map_err(|e| ClientError::SerdeJsonError(e.to_string()))?;

    let keep_alive = spawn_keep_alive_loop(blob.file_id);

    let path = std::path::Path::new(&blob_storage_dir).join(&blob.file_name);
    let offset_path = path.with_extension("offset");
    // if blob.needs_creation is true, we need to create the blob file
    let (mut blob_file, mut offset_file) = if blob.needs_creation {
        // check if the file exists already, if so panic
        if path.exists() || offset_path.exists() {
            panic!("Blob file already exists... this should never happen");
        }
        (
            tokio::fs::File::create(&path).await?,
            tokio::fs::File::create(&offset_path).await?,
        )
    } else {
        (
            // open in write mode.
            tokio::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .await?,
            // open in append mode
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&path.with_extension("offset"))
                .await?,
        )
    };
    // fseek to the offset given by the blob api
    blob_file
        .seek(std::io::SeekFrom::Start(blob.byte_offset))
        .await?;

    // write offset to the offset file
    offset_file
        .write_all(format!("\"{}\": {}\n", entries_keys, blob.byte_offset).as_bytes())
        .await?;

    // write files in order of the blob entries
    for bytes in blob_bytes {
        blob_file.write_all(&bytes).await?;
    }

    // unlock the blob
    let req = CreateUnlockRequest {
        file_id: blob.file_id,
        node_id,
    };

    let resp = client
        .post(format!("{}/blob/create_unlock", blob_api_url))
        .header("Authorization", blob_api_key)
        .json(&req)
        .send()
        .await?;

    // if we get a 200, we can continue
    check_req_failed(resp).await?;

    // kill keep alive loop
    keep_alive.abort();
    Ok(())
}

pub async fn store_from_local(args: Vec<String>) -> Result<(), ClientError> {
    if args.len() != 4 {
        eprintln!(
            "Usage: {} store <discovery node id> <tarball filepaths, separated by spaces>",
            args[0]
        );
        std::process::exit(1);
    }
    let node_id = args[2].clone();
    let filepaths = args[3]
        .split(' ')
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    // read all the files into memory
    let mut handles = vec![];
    for filepath in filepaths.clone() {
        handles.push(tokio::spawn(async move {
            let mut file = tokio::fs::File::open(&filepath).await?;
            let mut bytes = vec![];
            file.read_to_end(&mut bytes).await?;
            let filename = std::path::Path::new(&filepath)
                .file_name()
                .ok_or_else(|| ClientError::IoError("Bad filename".to_string()))?
                .to_str()
                .ok_or_else(|| ClientError::IoError("Bad filename".to_string()))?
                .to_string();
            Ok::<_, ClientError>((filename, bytes))
        }));
    }

    let mut blob_entries = vec![];
    let mut blob_bytes = vec![];
    let mut names = HashSet::new(); // can't have duplicate names
    for handle in handles {
        let (filename, bytes) = handle.await.unwrap()?;
        if !names.insert(filename.clone()) {
            continue;
        }
        let blob_entry = BlobEntry {
            key: filename,
            num_bytes: bytes.len() as u64,
        };
        blob_entries.push(blob_entry);
        blob_bytes.push(bytes);
    }

    store_into_blob(blob_entries, blob_bytes, node_id).await?;

    // delete the files
    for file in filepaths {
        tokio::fs::remove_file(file).await?;
    }

    Ok(())
}
