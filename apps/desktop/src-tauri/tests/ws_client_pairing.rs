use std::net::SocketAddr;
use std::sync::Arc;

use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::Mutex;
use worker_core::{build_router, AppState, PeerStore};

async fn spawn_test_worker() -> (SocketAddr, [u8; 32]) {
    let cert_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
    let cert_der = cert_key.cert.der().to_vec();
    let cert_pem = cert_key.cert.pem().into_bytes();
    let key_pem = cert_key.signing_key.serialize_pem().into_bytes();
    let tls_config = RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .expect("tls config");

    let peer_store = PeerStore::load(
        std::env::temp_dir().join(format!("desktop-test-peers-{}.json", std::process::id())),
    )
    .expect("peer store");
    let state = AppState {
        worker_id: "worker-desktop-test".to_string(),
        worker_name: "Test-Worker".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
        runtime_dir: std::env::temp_dir()
            .join(format!("desktop-test-runtime-{}", std::process::id())),
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

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    (addr, hasher.finalize().into())
}

#[tokio::test]
async fn pairs_over_a_real_tls_connection_and_pins_the_certificate() {
    desktop_lib::tls_verify::install_crypto_provider();
    let (addr, expected_fingerprint) = spawn_test_worker().await;

    let session = desktop_lib::ws_client::begin_pairing(
        &addr.ip().to_string(),
        addr.port(),
        "MacBook-Pro",
        "master-desktop-test",
    )
    .await
    .expect("begin_pairing");
    assert_eq!(session.fingerprint, expected_fingerprint);
    assert_eq!(session.code.len(), 6);

    let code = session.code.clone();
    let peer = desktop_lib::ws_client::confirm_pairing(session, code)
        .await
        .expect("confirm_pairing");
    assert_eq!(peer.worker_id, "worker-desktop-test");
    assert_eq!(peer.fingerprint, expected_fingerprint);
}

#[tokio::test]
async fn a_pinned_reconnect_to_a_different_certificate_is_rejected() {
    desktop_lib::tls_verify::install_crypto_provider();
    let (addr, _real_fingerprint) = spawn_test_worker().await;
    let wrong_peer = desktop_lib::peers::MasterPeer {
        worker_id: "worker-desktop-test".to_string(),
        worker_name: "Test-Worker".to_string(),
        host: addr.ip().to_string(),
        port: addr.port(),
        fingerprint: [0u8; 32],
        reconnect_token: "a".repeat(64),
    };

    let result = desktop_lib::ws_client::connect_paired(&wrong_peer).await;
    assert!(result.is_err());
}
