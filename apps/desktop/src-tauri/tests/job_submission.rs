use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;

use axum_server::tls_rustls::RustlsConfig;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use worker_core::{build_router, AppState, PeerStore};

fn init_fixture_repo(dir: &std::path::Path) {
    let repo = git2::Repository::init(dir).expect("init fixture repo");
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .expect("write");
    fs::create_dir_all(dir.join("src")).expect("mkdir");
    fs::write(
        dir.join("src").join("main.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .expect("write");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .expect("add_all");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("write_tree"))
        .expect("find_tree");
    let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .expect("commit");
    let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
    if branch_name != "main" {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("main", &commit, true).expect("branch");
        repo.set_head("refs/heads/main").expect("set_head");
    }
}

async fn spawn_test_worker() -> (SocketAddr, [u8; 32]) {
    let cert_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
    let cert_der = cert_key.cert.der().to_vec();
    let cert_pem = cert_key.cert.pem().into_bytes();
    let key_pem = cert_key.signing_key.serialize_pem().into_bytes();
    let tls_config = RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .expect("tls config");

    let peer_store = PeerStore::load(std::env::temp_dir().join(format!(
        "desktop-test-peers-{}-job-submit.json",
        std::process::id()
    )))
    .expect("peer store");
    let state = AppState {
        worker_id: "worker-desktop-job-test".to_string(),
        worker_name: "Test-Worker".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
        runtime_dir: std::env::temp_dir().join(format!(
            "desktop-test-worker-runtime-{}",
            std::process::id()
        )),
        jobs: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };
    let app = build_router(state);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");

    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls_config)
            .expect("from_tcp_rustls")
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .expect("server error");
    });

    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    (addr, hasher.finalize().into())
}

#[tokio::test]
async fn submits_a_job_over_a_pinned_connection_and_writes_the_reassembled_artifact() {
    desktop_lib::tls_verify::install_crypto_provider();

    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let (addr, fingerprint) = spawn_test_worker().await;
    let host = addr.ip().to_string();
    let port = addr.port();

    // Pair first, over a real handshake — this is also what populates the
    // worker's own peer_store, which the reconnect below depends on to
    // silently re-accept this master_id without a fresh code.
    let session =
        desktop_lib::ws_client::begin_pairing(&host, port, "MacBook-Pro", "master-job-submit-test")
            .await
            .expect("begin_pairing");
    assert_eq!(session.fingerprint, fingerprint);
    let code = session.code.clone();
    let mut peer = desktop_lib::ws_client::confirm_pairing(session, code)
        .await
        .expect("confirm_pairing");
    peer.host = host;
    peer.port = port;

    let stream = desktop_lib::ws_client::resume_paired_connection(
        &peer,
        "MacBook-Pro",
        "master-job-submit-test",
    )
    .await
    .expect("resume_paired_connection");

    struct NoopSink;
    impl desktop_lib::ws_client::JobEventSink for NoopSink {
        fn emit(&self, _event: &str, _message: &protocol::Message) {}
    }

    let artifacts_dir = tempfile::tempdir().expect("tempdir");
    let job_args = desktop_lib::ws_client::JobRequestArgs {
        job_id: "job-desktop-1".to_string(),
        repo: src.path().to_string_lossy().into_owned(),
        branch: "main".to_string(),
        profile: "debug".to_string(),
    };

    desktop_lib::ws_client::run_job_stream(
        stream,
        job_args,
        NoopSink,
        artifacts_dir.path().to_path_buf(),
        CancellationToken::new(),
    )
    .await
    .expect("run_job_stream");

    let job_dir = artifacts_dir.path().join("job-desktop-1");
    let entries: Vec<_> = fs::read_dir(&job_dir)
        .expect("artifact dir should exist")
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one artifact file");
    let artifact_bytes = fs::read(&entries[0]).expect("read artifact");
    assert!(!artifact_bytes.is_empty());
}
