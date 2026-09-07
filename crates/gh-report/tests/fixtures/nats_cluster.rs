use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

static SERIAL: Mutex<()> = Mutex::new(());

struct Nodes {
    children: Vec<Child>,
    storage: tempfile::TempDir,
    cleanup_errors: Arc<Mutex<Vec<String>>>,
}

impl Drop for Nodes {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
        }
        for child in &mut self.children {
            if let Err(error) = child.wait() {
                self.cleanup_errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("reap owned NATS child {}: {error}", child.id()));
            }
        }
    }
}

pub struct Cluster {
    urls: [String; 3],
    cleanup_errors: Arc<Mutex<Vec<String>>>,
    storage_path: std::path::PathBuf,
    stop: mpsc::Sender<()>,
    supervisor: Option<JoinHandle<()>>,
    _serial: MutexGuard<'static, ()>,
}

impl Cluster {
    pub fn acquire(test_name: &str) -> Option<Self> {
        pardosa_nats::test_support::probe_pinned_nats_server().ready_or_skip(test_name)?;
        let serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reservations: Vec<_> = (0..6)
            .map(|_| TcpListener::bind("127.0.0.1:0").expect("reserve local port"))
            .collect();
        let ports: Vec<_> = reservations
            .iter()
            .map(|listener| listener.local_addr().expect("local address").port())
            .collect();
        let urls = std::array::from_fn(|i| format!("nats://127.0.0.1:{}", ports[i]));
        let cleanup_errors = Arc::new(Mutex::new(Vec::new()));
        let mut nodes = Nodes {
            children: Vec::with_capacity(3),
            storage: tempfile::tempdir().expect("cluster storage"),
            cleanup_errors: Arc::clone(&cleanup_errors),
        };
        drop(reservations);
        for i in 0..3 {
            let routes = (0..3)
                .filter(|&j| j != i)
                .map(|j| format!("nats://127.0.0.1:{}", ports[3 + j]))
                .collect::<Vec<_>>()
                .join(",");
            nodes.children.push(
                Command::new("nats-server")
                    .args([
                        "-a",
                        "127.0.0.1",
                        "-p",
                        &ports[i].to_string(),
                        "-js",
                        "--name",
                        &format!("fence-{i}"),
                        "--cluster_name",
                        "fence-test",
                        "--cluster",
                        &format!("nats://127.0.0.1:{}", ports[3 + i]),
                        "--routes",
                        &routes,
                        "-sd",
                    ])
                    .arg(nodes.storage.path().join(i.to_string()))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("spawn owned cluster node"),
            );
        }
        let (stop, receiver) = mpsc::channel();
        let storage_path = nodes.storage.path().to_owned();
        let supervisor = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(180);
            let timed_out = loop {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break false,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                for child in &mut nodes.children {
                    let failure = match child.try_wait() {
                        Ok(None) => None,
                        Ok(Some(status)) => {
                            Some(format!("owned child {} exited: {status}", child.id()))
                        }
                        Err(error) => Some(format!(
                            "owned child {} health unknown: {error}",
                            child.id()
                        )),
                    };
                    if let Some(failure) = failure {
                        nodes
                            .cleanup_errors
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(failure);
                        return;
                    }
                }
                if std::time::Instant::now() >= deadline {
                    break true;
                }
            };
            drop(nodes);
            assert!(
                !timed_out,
                "cluster fixture exceeded 180s supervisor deadline"
            );
        });
        Some(Self {
            urls,
            cleanup_errors,
            storage_path,
            stop,
            supervisor: Some(supervisor),
            _serial: serial,
        })
    }

    pub fn url(&self) -> &str {
        &self.urls[0]
    }

    pub fn finish(self) {
        let errors = Arc::clone(&self.cleanup_errors);
        drop(self);
        let errors = errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(errors.is_empty(), "cluster cleanup: {errors:?}");
    }

    pub fn node_url(&self, index: usize) -> &str {
        &self.urls[index % 3]
    }

    pub fn ready(&self) {
        self.ready_with_placement(async |diagnosis| self.placement_barrier(diagnosis).await);
    }

    fn ready_with_placement(&self, placement: impl AsyncFnOnce(&mut String)) {
        let rt = tokio::runtime::Runtime::new().expect("readiness runtime");
        let mut diagnosis = "pre-open endpoint connectivity".to_owned();
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                let initialization = async {
                for (i, url) in self.urls.iter().enumerate() {
                    loop {
                        if let Ok(client) = async_nats::ConnectOptions::new()
                            .retry_on_initial_connect()
                            .connect(url)
                            .await
                        {
                            client.flush().await.expect("readiness round trip");
                            assert_eq!(client.server_info().server_name, format!("fence-{i}"));
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                placement(&mut diagnosis).await;
                };
                tokio::pin!(initialization);
                loop {
                    tokio::select! {
                        () = &mut initialization => break,
                        () = tokio::time::sleep(Duration::from_millis(100)) => {
                            let errors = self.cleanup_errors.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            assert!(errors.is_empty(), "pre-open owned child health: {errors:?}");
                        }
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("pre-open readiness exceeded shared 30s deadline: {diagnosis}"));
        });
    }

    async fn placement_barrier(&self, diagnosis: &mut String) {
        let name = format!(
            "fixture_placement_{}",
            self.storage_path
                .file_name()
                .expect("owned storage name")
                .to_string_lossy()
                .replace('.', "_")
        );
        let client = retry_stage(diagnosis, "placement connect", || {
            async_nats::connect(self.url())
        })
        .await;
        let js = async_nats::jetstream::new(client);
        retry_stage(diagnosis, "placement create R3", || {
            js.create_stream(async_nats::jetstream::stream::Config {
                name: name.clone(),
                num_replicas: 3,
                max_messages: 1,
                max_bytes: 1024,
                ..Default::default()
            })
        })
        .await;
        for url in &self.urls {
            let client = retry_stage(diagnosis, &format!("placement {url} connect"), || {
                async_nats::connect(url)
            })
            .await;
            let js = async_nats::jetstream::new(client);
            let stream = retry_stage(diagnosis, &format!("placement {url} stream"), || {
                js.get_stream(&name)
            })
            .await;
            loop {
                let info = retry_stage(diagnosis, &format!("placement {url} info"), || {
                    stream.get_info()
                })
                .await;
                if replication_converged(&info) {
                    break;
                }
                *diagnosis = format!("placement {url} quorum/current: {:?}", info.cluster);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        retry_stage(diagnosis, "placement delete owned probe", || {
            js.delete_stream(&name)
        })
        .await;
    }

    pub async fn replicate(&self, stream_name: &str) {
        let mut diagnosis = String::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            let client = retry_stage(&mut diagnosis, "cluster connect", || {
                async_nats::connect(self.url())
            })
            .await;
            let js = async_nats::jetstream::new(client);
            let stream = retry_stage(&mut diagnosis, "cluster get_stream", || {
                js.get_stream(stream_name)
            })
            .await;
            let mut config = retry_stage(&mut diagnosis, "cluster get_info", || stream.get_info())
                .await
                .config;
            config.num_replicas = 3;
            retry_stage(&mut diagnosis, "update_stream", || {
                js.update_stream(config.clone())
            })
            .await;
            for url in &self.urls {
                let client = retry_stage(&mut diagnosis, &format!("{url} connect"), || {
                    async_nats::connect(url)
                })
                .await;
                let js = async_nats::jetstream::new(client);
                let stream = retry_stage(&mut diagnosis, &format!("{url} get_stream"), || {
                    js.get_stream(stream_name)
                })
                .await;
                loop {
                    let info = retry_stage(&mut diagnosis, &format!("{url} get_info"), || {
                        stream.get_info()
                    })
                    .await;
                    if replication_converged(&info) {
                        break;
                    }
                    diagnosis = format!(
                        "{url} convergence: replicas={}, cluster={:?}",
                        info.config.num_replicas, info.cluster
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("three current stream peers within 30s: {diagnosis}"));
    }
}

#[test]
fn connected_endpoints_do_not_open_before_placement() {
    let Some(server) = Cluster::acquire("connected_endpoints_do_not_open_before_placement") else {
        return;
    };
    let placement_completed = std::cell::Cell::new(false);
    server.ready_with_placement(async |_| {
        tokio::time::sleep(Duration::from_millis(150)).await;
        placement_completed.set(true);
    });
    assert!(
        placement_completed.get(),
        "initial open must follow placement, not just client connectivity"
    );
    server.finish();
}

#[test]
fn initialization_calls_placement_before_production_open() {
    let fixture = include_str!("nats_cluster.rs");
    let ready = fixture
        .split("pub fn ready(&self) {")
        .nth(1)
        .expect("ready function")
        .split("fn ready_with_placement")
        .next()
        .expect("ready body");
    assert!(ready.contains("self.placement_barrier(diagnosis).await"));
    for (source, scenario) in [
        (
            include_str!("../live_nats_n_writer_fence_property.rs"),
            "fn n_writers_race_real_fence(n: usize)",
        ),
        (
            include_str!("../live_nats_n_writer_fence_property.rs"),
            "fn single_handle_two_racing_appends_never_self_fence()",
        ),
        (
            include_str!("two_writer_fence.rs"),
            "fn two_writer_fence_conflicts_loser_and_single_writer_handles_sync_and_purge()",
        ),
    ] {
        let body = source.split(scenario).nth(1).expect("scenario body");
        assert!(
            body.find("server.ready();").expect("ready call")
                < body.find("open_state(").expect("initial production open")
        );
    }
}

async fn retry_stage<T, E: std::fmt::Display, F: Future<Output = Result<T, E>>>(
    diagnosis: &mut String,
    stage: &str,
    mut operation: impl FnMut() -> F,
) -> T {
    stage.clone_into(diagnosis);
    loop {
        match operation().await {
            Ok(value) => return value,
            Err(error) => *diagnosis = format!("{stage}: {error}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn transient_retries_preserve_deadline_and_diagnosis() {
    let mut diagnosis = String::new();
    let result = tokio::time::timeout(Duration::from_millis(250), async {
        retry_stage(&mut diagnosis, "get_info", || {
            std::future::ready(Err::<(), _>("forced transient"))
        })
        .await;
    })
    .await;
    assert!(result.is_err());
    assert_eq!(diagnosis, "get_info: forced transient");
}

#[tokio::test]
async fn transient_stages_retry_then_converge() {
    for stage in ["connect", "get_stream", "get_info", "update_stream"] {
        let mut attempts = 0;
        let mut diagnosis = String::new();
        let value = retry_stage(&mut diagnosis, stage, || {
            attempts += 1;
            std::future::ready(if attempts == 1 {
                Err("forced transient")
            } else {
                Ok(3)
            })
        })
        .await;
        assert_eq!((value, attempts), (3, 2), "{stage} must retry");
    }
}

fn replication_converged(info: &async_nats::jetstream::stream::Info) -> bool {
    info.config.num_replicas == 3
        && info.cluster.as_ref().is_some_and(|cluster| {
            let mut names: Vec<_> = cluster.leader.iter().map(String::as_str).collect();
            names.extend(cluster.replicas.iter().map(|peer| peer.name.as_str()));
            names.sort_unstable();
            names == ["fence-0", "fence-1", "fence-2"]
                && cluster
                    .replicas
                    .iter()
                    .all(|peer| peer.current && !peer.offline)
        })
}

#[test]
fn premature_replicas_retry_until_convergence() {
    let mut info: async_nats::jetstream::stream::Info = serde_json::from_value(serde_json::json!({
        "config": async_nats::jetstream::stream::Config { name: "fixture".to_owned(), num_replicas: 1, ..Default::default() },
        "created": "2026-09-06T00:00:00Z",
        "state": {"messages": 0, "bytes": 0, "first_seq": 0, "first_ts": "2026-09-06T00:00:00Z", "last_seq": 0, "last_ts": "2026-09-06T00:00:00Z", "consumer_count": 0},
        "cluster": {"leader": "fence-0", "replicas": [
            {"name": "fence-1", "current": true, "offline": false, "active": 0},
            {"name": "fence-2", "current": true, "offline": false, "active": 0}
        ]}
    })).expect("stream info fixture");
    assert!(
        !replication_converged(&info),
        "premature replica count must retry, not panic"
    );
    info.config.num_replicas = 3;
    assert!(replication_converged(&info));
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(supervisor) = self.supervisor.take()
            && supervisor.join().is_err()
        {
            self.cleanup_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("cluster supervisor failed".to_owned());
        }
        let mut errors = self
            .cleanup_errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.storage_path.exists() {
            errors.push("cluster storage not removed".to_owned());
        }
        if !errors.is_empty() {
            use std::io::Write;
            let _ = writeln!(std::io::stderr().lock(), "cluster cleanup: {errors:?}");
        }
    }
}

#[test]
fn cluster_cleanup_on_startup_and_ready_unwind() {
    for ready in [false, true] {
        let Some(server) = Cluster::acquire("cluster_cleanup_on_startup_and_ready_unwind") else {
            return;
        };
        let storage = server.storage_path.clone();
        let urls = server.urls.clone();
        let errors = Arc::clone(&server.cleanup_errors);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let server = server;
            if ready {
                server.ready();
            }
            panic!("injected fixture failure");
        }));
        assert!(result.is_err());
        assert!(
            errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "cluster cleanup errors: {errors:?}"
        );
        assert!(!storage.exists(), "cluster storage removed");
        for url in urls {
            let address = url.strip_prefix("nats://").expect("local URL");
            let listener = TcpListener::bind(address).expect("owned client port released");
            drop(listener);
        }
    }
}
