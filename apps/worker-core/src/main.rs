use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::Mutex;
use worker_core::{build_router, load_or_generate_cert, AppState, PeerStore};

const WORKER_PORT: u16 = 7878;

fn runtime_dir() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("DistBuild")
}

fn worker_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "distbuild-worker".to_string())
}

fn worker_id() -> String {
    format!("worker-{:016x}", rand::random::<u64>())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let runtime_dir = runtime_dir();
    let cert_pems = load_or_generate_cert(&runtime_dir.join("tls"))
        .expect("failed to load or generate the worker's TLS certificate");
    let tls_config = RustlsConfig::from_pem(cert_pems.cert_pem, cert_pems.key_pem)
        .await
        .expect("failed to build TLS config from the worker's certificate");

    let peer_store =
        PeerStore::load(runtime_dir.join("peers.json")).expect("failed to load peers.json");

    let state = AppState {
        worker_id: worker_id(),
        worker_name: worker_hostname(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
    };

    tracing::info!(
        worker_id = %state.worker_id,
        worker_name = %state.worker_name,
        "worker-core: headless worker daemon starting"
    );

    let app = build_router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], WORKER_PORT));
    tracing::info!(%addr, "listening for Master connections over wss://.../ws");

    axum_server::tls_rustls::bind_rustls(addr, tls_config)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("worker-core server error");
}
