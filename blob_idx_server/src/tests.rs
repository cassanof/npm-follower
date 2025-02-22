use std::time::Duration;

use lazy_static::lazy_static;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::{
    blob::{BlobOffset, BlobStorageConfig, BlobStorageSlice},
    errors::{BlobError, JobError},
    http::{
        BlobEntry, CreateAndLockRequest, CreateUnlockRequest, KeepAliveLockRequest, LookupRequest,
        HTTP,
    },
    job::JobManagerConfig,
    ssh::{Ssh, SshFactory},
};

lazy_static! {
    static ref GLOBAL_LOCK: Mutex<()> = Mutex::new(());
    // NOTE: we use db 5 for testing. make sure you don't have anything important there.
    static ref REDIS_TEST_DB: &'static str = "redis://127.0.0.1/5";
}

macro_rules! blob_test {
    ($body:block, $cfg:expr) => {
        let _lock = GLOBAL_LOCK.lock().await;
        redis_cleanup();
        let server = run_test_server($cfg).await;

        $body;

        server.shutdown().await;
        drop(_lock);
    };
    // default config
    ($body:block) => {
        blob_test!($body, make_config(1, 5)); // we have 1 file max
    };
}

fn make_config(max_files: u32, lock_timeout: u64) -> BlobStorageConfig {
    BlobStorageConfig {
        redis_url: REDIS_TEST_DB.to_string(),
        max_files,
        lock_timeout,
    }
}

// fn simple_config() -> BlobStorageConfig {
//     make_config(2, 5)
// }

fn redis_cleanup() {
    let client = bb8_redis::redis::Client::open(REDIS_TEST_DB.to_string()).unwrap();
    let mut con = client.get_connection().unwrap();
    bb8_redis::redis::cmd("FLUSHDB")
        .query::<()>(&mut con)
        .unwrap();
}

struct TestServer {
    shutdown_signal: tokio::sync::mpsc::Sender<()>,
    handle: JoinHandle<()>,
}

#[derive(Debug, Clone)]
struct FakeSshFactory {}

#[derive(Debug, Clone)]
struct FakeSsh {}

#[async_trait::async_trait]
impl SshFactory for FakeSshFactory {
    async fn spawn(&self) -> Result<Box<dyn Ssh>, JobError> {
        Ok(Box::new(FakeSsh {}) as Box<dyn Ssh>)
    }

    async fn spawn_jumped(&self, _jump_to: &str) -> Result<Box<dyn Ssh>, JobError> {
        Ok(Box::new(FakeSsh {}) as Box<dyn Ssh>)
    }
}

#[async_trait::async_trait]
impl Ssh for FakeSsh {
    async fn connect(_ssh_user_host: &str) -> Result<Self, JobError> {
        Ok(FakeSsh {})
    }

    async fn connect_jumped(_ssh_user_host: &str, _jump_to: &str) -> Result<Self, JobError> {
        Ok(FakeSsh {})
    }

    async fn run_command(&self, _cmd: &str) -> Result<String, JobError> {
        if _cmd.contains("sbatch") {
            Ok("123".to_string())
        } else {
            Ok("".to_string())
        }
    }
}

impl TestServer {
    async fn shutdown(self) {
        self.shutdown_signal.try_send(()).unwrap();
        self.handle.await.unwrap();
    }
}

async fn run_test_server(cfg: BlobStorageConfig) -> TestServer {
    let http = HTTP::new(
        "127.0.0.1".to_string(),
        "1337".to_string(),
        "123".to_string(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);

    let task = tokio::spawn(async move {
        http.start(
            cfg.clone(),
            JobManagerConfig {
                ssh_factory: Box::new(FakeSshFactory {}),
                max_comp_worker_jobs: 1,
                max_xfer_worker_jobs: 1,
            },
            async move {
                rx.recv().await;
            },
        )
        .await
        .unwrap()
    });

    // wait for the server to start
    while (reqwest::get("http://127.0.0.1:1337/").await).is_err() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    TestServer {
        shutdown_signal: tx,
        handle: task,
    }
}

async fn send_create_and_lock_request(
    client: &reqwest::Client,
    req: CreateAndLockRequest,
) -> Result<BlobOffset, BlobError> {
    let resp = client
        .post("http://127.0.0.1:1337/blob/create_and_lock")
        .body(serde_json::to_string(&req).unwrap())
        .header("Authorization", "123")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    serde_json::from_str::<BlobOffset>(&resp).map_err(|_| {
        let json_map =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&resp).unwrap();
        let err = json_map.get("error").unwrap();
        serde_json::from_value::<BlobError>(err.clone()).unwrap()
    })
}

async fn send_create_unlock_request(
    client: &reqwest::Client,
    req: CreateUnlockRequest,
) -> (reqwest::StatusCode, Option<BlobError>) {
    let resp = client
        .post("http://127.0.0.1:1337/blob/create_unlock")
        .body(serde_json::to_string(&req).unwrap())
        .header("Authorization", "123")
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body = resp.text().await.unwrap();

    let err = if status.is_success() {
        None
    } else {
        let json_map =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&body).unwrap();
        let err = json_map.get("error").unwrap();
        Some(serde_json::from_value::<BlobError>(err.clone()).unwrap())
    };

    (status, err)
}

async fn send_keepalive_request(
    client: &reqwest::Client,
    req: KeepAliveLockRequest,
) -> (reqwest::StatusCode, Option<BlobError>) {
    let resp = client
        .post("http://127.0.0.1:1337/blob/keep_alive_lock")
        .body(serde_json::to_string(&req).unwrap())
        .header("Authorization", "123")
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body = resp.text().await.unwrap();

    let err = if status.is_success() {
        None
    } else {
        let json_map =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&body).unwrap();
        let err = json_map.get("error").unwrap();
        Some(serde_json::from_value::<BlobError>(err.clone()).unwrap())
    };

    (status, err)
}

async fn send_lookup_request(
    client: &reqwest::Client,
    req: LookupRequest,
) -> Result<BlobStorageSlice, BlobError> {
    let resp = client
        .get("http://127.0.0.1:1337/blob/lookup")
        .body(serde_json::to_string(&req).unwrap())
        .header("Authorization", "123")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    serde_json::from_str::<BlobStorageSlice>(&resp).map_err(|_| {
        let json_map =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&resp).unwrap();
        let err = json_map.get("error").unwrap();
        serde_json::from_value::<BlobError>(err.clone()).unwrap()
    })
}

#[tokio::test]
async fn test_simple_get_slice_unlock_lookup() {
    let client = reqwest::Client::new();
    blob_test!({
        let offset = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![
                    BlobEntry::new("k1".to_string(), 1),
                    BlobEntry::new("k2".to_string(), 2),
                ],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(offset.file_id, 0); // picks the first file
        assert_eq!(offset.byte_offset, 0); // starts at 0

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        // unlock
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: offset.file_id,
                node_id: "n1".to_string(),
            },
        )
        .await;
        assert_eq!(resp.0, 200);

        // lookup
        let slice = send_lookup_request(
            &client,
            LookupRequest {
                key: "k1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(slice.file_id, 0);
        assert_eq!(slice.byte_offset, 0);
        assert_eq!(slice.num_bytes, 1);

        // lock again
        let offset = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![
                    BlobEntry::new("k3".to_string(), 1),
                    BlobEntry::new("k4".to_string(), 2),
                ],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(offset.file_id, 0); // picks the first file
        assert_eq!(offset.byte_offset, 3); // starts at 3

        // unlock
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: offset.file_id,
                node_id: "n1".to_string(),
            },
        )
        .await;
        assert_eq!(resp.0, 200);

        // lookup
        let slice = send_lookup_request(
            &client,
            LookupRequest {
                key: "k4".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(slice.file_id, 0);
        assert_eq!(slice.byte_offset, 4);
        assert_eq!(slice.num_bytes, 2);
    });
}

#[tokio::test]
/// This tests what happens when all files are locked, and multiple nodes try to lock
async fn test_lock_wait() {
    let client = reqwest::Client::new();
    blob_test!({
        // n1 initially locks, gets the lock
        let client1 = client.clone();
        let handle1 = tokio::task::spawn(async move {
            let now = std::time::Instant::now();
            let res = send_create_and_lock_request(
                &client1,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k1".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await;
            (res, now.elapsed())
        });

        // n2 waits for lock
        let client2 = client.clone();
        let handle2 = tokio::task::spawn(async move {
            let now = std::time::Instant::now();
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            let res = send_create_and_lock_request(
                &client2,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k2".to_string(), 3)],
                    node_id: "n2".to_string(),
                },
            )
            .await;
            (res, now.elapsed())
        });

        // n3 waits for lock
        let client3 = client.clone();
        let handle3 = tokio::task::spawn(async move {
            let now = std::time::Instant::now();
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let res = send_create_and_lock_request(
                &client3,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k3".to_string(), 10)],
                    node_id: "n3".to_string(),
                },
            )
            .await;
            (res, now.elapsed())
        });

        // wait for first
        let (o1, time1) = handle1.await.unwrap();
        let o1 = o1.unwrap();

        // unlock first
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 200);

        // wait for second
        let (o2, time2) = handle2.await.unwrap();
        let o2 = o2.unwrap();

        // unlock n2
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n2".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 200);

        // wait for third
        let (o3, time3) = handle3.await.unwrap();
        let o3 = o3.unwrap();

        // unlock n3
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n3".to_string(),
            },
        )
        .await;

        // check that they indeed waited
        // assert!(time1 < time2);
        // assert!(time2 < time3);
        println!("time1: {:?}", time1);
        println!("time2: {:?}", time2);
        println!("time3: {:?}", time3);

        assert_eq!(resp.0, 200);

        // check offsets (needs creation)
        assert_eq!(o1.file_id, 0);
        assert!(o1.needs_creation);
        assert_eq!(o1.byte_offset, 0);

        assert_eq!(o2.file_id, 0);
        assert!(!o2.needs_creation);
        assert_eq!(o2.byte_offset, 1);

        assert_eq!(o3.file_id, 0);
        assert!(!o3.needs_creation);
        assert_eq!(o3.byte_offset, 4);

        // -------------------------------------------------------
        // now that everything is unlocked, we redo the same thing
        // -------------------------------------------------------

        // n1 initially locks, gets the lock
        let client1 = client.clone();
        let handle1 = tokio::task::spawn(async move {
            send_create_and_lock_request(
                &client1,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k4".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
        });

        // n2 waits for lock
        let client2 = client.clone();
        let handle2 = tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            send_create_and_lock_request(
                &client2,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k5".to_string(), 3)],
                    node_id: "n2".to_string(),
                },
            )
            .await
        });

        // n3 waits for lock
        let client3 = client.clone();
        let handle3 = tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            send_create_and_lock_request(
                &client3,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k6".to_string(), 10)],
                    node_id: "n3".to_string(),
                },
            )
            .await
        });

        // wait for first
        let o1 = handle1.await.unwrap().unwrap();

        // unlock first
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 200);

        // wait for second
        let o2 = handle2.await.unwrap().unwrap();

        // unlock n2
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n2".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 200);

        // wait for third
        let o3 = handle3.await.unwrap().unwrap();

        // unlock n3
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: 0,
                node_id: "n3".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 200);

        // check offsets (now doesn't need creation)
        assert_eq!(o1.file_id, 0);
        assert!(!o1.needs_creation);
        assert_eq!(o1.byte_offset, 14);

        assert_eq!(o2.file_id, 0);
        assert!(!o2.needs_creation);
        assert_eq!(o2.byte_offset, 15);

        assert_eq!(o3.file_id, 0);
        assert!(!o3.needs_creation);
        assert_eq!(o3.byte_offset, 18);
    });
}

#[tokio::test]
async fn test_cleaner_basic() {
    let client = reqwest::Client::new();
    let cfg = make_config(1, 2); // two seconds to clean
    blob_test!(
        {
            // lock n1
            let resp1 = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k1".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
            .unwrap();

            // wait two seconds
            tokio::time::sleep(Duration::from_secs(2)).await;

            // lock another one, should be same file.
            let resp2 = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k2".to_string(), 1)],
                    node_id: "n2".to_string(),
                },
            )
            .await
            .unwrap();

            // check that it's the same file
            assert_eq!(resp1.file_id, resp2.file_id);

            // unlock n2

            let resp = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp2.file_id,
                    node_id: "n2".to_string(),
                },
            )
            .await;

            assert_eq!(resp.0, 200);

            // ok now let's test for keepalive (at least 3)

            // lock n1

            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k3".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
            .unwrap();

            // wait some time
            tokio::time::sleep(Duration::from_millis(250)).await;

            // keep alive
            let keep1 = send_keepalive_request(
                &client,
                KeepAliveLockRequest {
                    file_id: resp.file_id,
                },
            )
            .await;
            assert_eq!(keep1.0, 200);

            // wait some time
            tokio::time::sleep(Duration::from_millis(250)).await;

            // keep alive

            let keep2 = send_keepalive_request(
                &client,
                KeepAliveLockRequest {
                    file_id: resp.file_id,
                },
            )
            .await;
            assert_eq!(keep2.0, 200);

            // wait some time
            tokio::time::sleep(Duration::from_millis(250)).await;

            // keep alive

            let keep3 = send_keepalive_request(
                &client,
                KeepAliveLockRequest {
                    file_id: resp.file_id,
                },
            )
            .await;
            assert_eq!(keep3.0, 200);

            // now check that we can unlock manually

            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n1".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);

            // now lets keepalive, but not unlock

            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k4".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
            .unwrap();

            // wait some time
            tokio::time::sleep(Duration::from_millis(250)).await;

            // keep alive

            let keep1 = send_keepalive_request(
                &client,
                KeepAliveLockRequest {
                    file_id: resp.file_id,
                },
            )
            .await;

            assert_eq!(keep1.0, 200);

            // wait two seconds

            tokio::time::sleep(Duration::from_secs(2)).await;

            // check that we can't unlock

            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n1".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 400);

            // check k4 not written
            let resp = send_lookup_request(
                &client,
                LookupRequest {
                    key: "k4".to_string(),
                },
            )
            .await;

            assert!(resp.is_err());
            assert_eq!(resp.unwrap_err(), BlobError::NotWritten);
        },
        cfg
    );
}

/// This tests if there exists a race condition between cleaning and keepalive.
/// Runs tasks in parallel, one that keeps a lock alive, and another that cleans
#[tokio::test]
async fn test_cleaner_keepalive_race() {
    let client = reqwest::Client::new();
    let cfg = make_config(1, 1); // zero second to clean
    let epochs = 20;
    blob_test!(
        {
            let mut handles = vec![];
            for i in 0..epochs {
                let client = client.clone();
                handles.push(tokio::task::spawn(async move {
                    // lock
                    let resp = send_create_and_lock_request(
                        &client,
                        CreateAndLockRequest {
                            entries: vec![BlobEntry::new(format!("k{}", i), 1)],
                            node_id: "n1".to_string(),
                        },
                    )
                    .await
                    .unwrap();

                    // send keepalive, unlock

                    let keep = send_keepalive_request(
                        &client,
                        KeepAliveLockRequest {
                            file_id: resp.file_id,
                        },
                    )
                    .await;

                    assert_eq!(keep.0, 200);

                    let u = send_create_unlock_request(
                        &client,
                        CreateUnlockRequest {
                            file_id: resp.file_id,
                            node_id: "n1".to_string(),
                        },
                    )
                    .await;

                    assert_eq!(u.0, 200);
                }));
            }

            for h in handles {
                h.await.unwrap();
            }
        },
        cfg
    );
}

#[tokio::test]
async fn more_than_one_file() {
    let client = reqwest::Client::new();
    let cfg = make_config(3, 20); // 3 files

    blob_test!(
        {
            // send h1
            let client_cl = client.clone();
            let h1 = tokio::task::spawn(async move {
                send_create_and_lock_request(
                    &client_cl,
                    CreateAndLockRequest {
                        entries: vec![BlobEntry::new("k1".to_string(), 1)],
                        node_id: "n1".to_string(),
                    },
                )
                .await
            });
            // send h2
            let client_cl = client.clone();
            let h2 = tokio::task::spawn(async move {
                send_create_and_lock_request(
                    &client_cl,
                    CreateAndLockRequest {
                        entries: vec![BlobEntry::new("k2".to_string(), 1)],
                        node_id: "n1".to_string(),
                    },
                )
                .await
            });
            // send h3
            let client_cl = client.clone();
            let h3 = tokio::task::spawn(async move {
                send_create_and_lock_request(
                    &client_cl,
                    CreateAndLockRequest {
                        entries: vec![BlobEntry::new("k3".to_string(), 1)],
                        node_id: "n1".to_string(),
                    },
                )
                .await
            });

            // only unlock one of them
            let resp = h2.await.unwrap().unwrap();
            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n1".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);

            // try to lock another one, should be the same as resp.file_id

            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k4".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
            .unwrap();

            assert_eq!(resp.file_id, resp.file_id);

            // unlock all
            let resp = h1.await.unwrap().unwrap();
            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n1".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);

            let resp = h3.await.unwrap().unwrap();
            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n1".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);
        },
        cfg
    );
}

#[tokio::test]
async fn test_not_written() {
    let client = reqwest::Client::new();
    blob_test!({
        // lock a file
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(resp.file_id, 0);

        // try to lookup before unlock
        let resp = send_lookup_request(
            &client,
            LookupRequest {
                key: "k1".to_string(),
            },
        )
        .await;

        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), BlobError::NotWritten);
    });
}

#[tokio::test]
async fn test_wrong_node_id() {
    let client = reqwest::Client::new();
    blob_test!({
        // lock with n1
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(resp.file_id, 0);

        // try to unlock with n2
        let resp = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: resp.file_id,
                node_id: "n2".to_string(),
            },
        )
        .await;

        assert_eq!(resp.0, 400);
        assert_eq!(resp.1.unwrap(), BlobError::WrongNode);
    });
}

#[tokio::test]
async fn test_slice_doesnt_exist() {
    let client = reqwest::Client::new();
    blob_test!({
        // lookup a slice that doesn't exist
        let resp = send_lookup_request(
            &client,
            LookupRequest {
                key: "k1".to_string(),
            },
        )
        .await;

        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), BlobError::DoesNotExist("k1".to_string()));
    });
}

#[tokio::test]
async fn test_unlock_twice() {
    let client = reqwest::Client::new();
    blob_test!({
        // lock a file
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(resp.file_id, 0);

        // unlock it
        let u = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: resp.file_id,
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert_eq!(u.0, 200);

        // unlock it again
        let u = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: resp.file_id,
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert_eq!(u.0, 400);
        assert_eq!(u.1.unwrap(), BlobError::CreateNotLocked);
    });
}

#[tokio::test]
async fn test_duplicate_keys() {
    let client = reqwest::Client::new();
    blob_test!({
        // lock a file
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![
                    BlobEntry::new("k1".to_string(), 1),
                    BlobEntry::new("k1".to_string(), 1),
                ],
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), BlobError::DuplicateKeys);
    });
}

#[tokio::test]
async fn test_already_created_key() {
    let client = reqwest::Client::new();
    blob_test!({
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(resp.file_id, 0);

        // unlock

        let u = send_create_unlock_request(
            &client,
            CreateUnlockRequest {
                file_id: resp.file_id,
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert_eq!(u.0, 200);

        // try to create again
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert!(resp.is_err());
        assert_eq!(
            resp.unwrap_err(),
            BlobError::AlreadyExists("k1".to_string())
        );
    });
}

#[tokio::test]
async fn test_lock_twice() {
    let client = reqwest::Client::new();
    blob_test!({
        // lock key "k1"
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(resp.file_id, 0);

        // lock same key again
        let resp = send_create_and_lock_request(
            &client,
            CreateAndLockRequest {
                entries: vec![BlobEntry::new("k1".to_string(), 1)],
                node_id: "n1".to_string(),
            },
        )
        .await;

        assert!(resp.is_err());
        assert_eq!(
            resp.unwrap_err(),
            BlobError::AlreadyExists("k1".to_string())
        );
    });
}

/// This tests what happens when a node locks, then it gets cleaned, and and another node
/// wants to lock the same key.
#[tokio::test]
async fn test_rewrite_cleaned() {
    let client = reqwest::Client::new();
    let cfg = make_config(1, 2);

    blob_test!(
        {
            // lock a file
            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k1".to_string(), 1)],
                    node_id: "n1".to_string(),
                },
            )
            .await
            .unwrap();

            assert_eq!(resp.file_id, 0);
            assert_eq!(resp.byte_offset, 0);

            // wait for the lock to expire
            tokio::time::sleep(Duration::from_secs(2)).await;

            // lock another different key

            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k2".to_string(), 1)],
                    node_id: "n2".to_string(),
                },
            )
            .await
            .unwrap();

            assert_eq!(resp.file_id, 0);
            assert_eq!(resp.byte_offset, 1);

            // unlock it
            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n2".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);

            // now, try to lock the first key again

            let resp = send_create_and_lock_request(
                &client,
                CreateAndLockRequest {
                    entries: vec![BlobEntry::new("k1".to_string(), 1)],
                    node_id: "n2".to_string(),
                },
            )
            .await
            .unwrap();

            assert_eq!(resp.file_id, 0);
            assert_eq!(resp.byte_offset, 2); // NOTE: important, it wouldn't be 0

            // unlock
            let u = send_create_unlock_request(
                &client,
                CreateUnlockRequest {
                    file_id: resp.file_id,
                    node_id: "n2".to_string(),
                },
            )
            .await;

            assert_eq!(u.0, 200);
        },
        cfg
    );
}
