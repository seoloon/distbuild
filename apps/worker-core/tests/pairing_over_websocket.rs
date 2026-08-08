//! End-to-end check that the pairing flow works over a real WebSocket
//! connection (not just against the in-memory `PairingSession` state
//! machine): binds `build_router` on a real localhost TCP port and drives
//! it with a genuine `tokio-tungstenite` client, round-tripping
//! `protocol::Message` JSON frames exactly as a real Master would.
//!
//! This deliberately uses plain `ws://` rather than `wss://`: TLS
//! termination happens at the `axum_server::tls_rustls::bind_rustls` layer
//! in `main.rs`, which `build_router` is agnostic to (see its doc comment).
//! `crates::tls` already has its own dedicated tests for certificate
//! generation and reuse.

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use protocol::Message;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use worker_core::{AppState, PeerStore};

async fn spawn_test_server() -> SocketAddr {
    let peer_store = PeerStore::load(
        std::env::temp_dir().join(format!("distbuild-test-peers-{}.json", std::process::id())),
    )
    .expect("load peer store");

    let state = AppState {
        worker_id: "worker-test-1".to_string(),
        worker_name: "Test-Worker".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
    };

    let app = worker_core::build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("server error");
    });

    addr
}

#[tokio::test]
async fn full_pairing_flow_over_a_real_websocket_connection() {
    let addr = spawn_test_server().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");

    let pair_request = Message::PairRequest {
        master_name: "MacBook-Pro".to_string(),
        master_id: "master-e2e-test".to_string(),
    };
    ws.send(WsMessage::Text(
        serde_json::to_string(&pair_request).unwrap().into(),
    ))
    .await
    .expect("send PairRequest");

    let challenge = next_message(&mut ws).await;
    let code = match challenge {
        Message::PairChallenge {
            code_shown_on_worker,
        } => code_shown_on_worker,
        other => panic!("expected PairChallenge, got {other:?}"),
    };
    assert_eq!(code.len(), 6);

    let confirm = Message::PairConfirm { code };
    ws.send(WsMessage::Text(
        serde_json::to_string(&confirm).unwrap().into(),
    ))
    .await
    .expect("send PairConfirm");

    let accepted = next_message(&mut ws).await;
    match accepted {
        Message::PairAccepted {
            worker_id,
            worker_name,
            os,
            arch,
        } => {
            assert_eq!(worker_id, "worker-test-1");
            assert_eq!(worker_name, "Test-Worker");
            assert_eq!(os, "linux");
            assert_eq!(arch, "x86_64");
        }
        other => panic!("expected PairAccepted, got {other:?}"),
    }
}

#[tokio::test]
async fn wrong_code_over_a_real_websocket_connection_is_rejected() {
    let addr = spawn_test_server().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");

    let pair_request = Message::PairRequest {
        master_name: "MacBook-Pro".to_string(),
        master_id: "master-e2e-test-2".to_string(),
    };
    ws.send(WsMessage::Text(
        serde_json::to_string(&pair_request).unwrap().into(),
    ))
    .await
    .expect("send PairRequest");
    next_message(&mut ws).await; // PairChallenge, code discarded on purpose

    let confirm = Message::PairConfirm {
        code: "000000".to_string(),
    };
    ws.send(WsMessage::Text(
        serde_json::to_string(&confirm).unwrap().into(),
    ))
    .await
    .expect("send PairConfirm");

    let reply = next_message(&mut ws).await;
    match reply {
        Message::Error { code, .. } => assert_eq!(code, "PAIRING_FAILED"),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn repeated_wrong_codes_lock_and_close_the_connection() {
    let addr = spawn_test_server().await;
    let url = format!("ws://{addr}/ws");

    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");

    // worker_core::pairing::MAX_PAIRING_ATTEMPTS is private to the crate;
    // 5 is duplicated here deliberately so this test breaks loudly (rather
    // than silently under-testing) if that constant ever changes.
    const MAX_ATTEMPTS: usize = 5;

    for attempt in 1..=MAX_ATTEMPTS {
        let pair_request = Message::PairRequest {
            master_name: "MacBook-Pro".to_string(),
            master_id: "master-lockout-test".to_string(),
        };
        ws.send(WsMessage::Text(
            serde_json::to_string(&pair_request).unwrap().into(),
        ))
        .await
        .expect("send PairRequest");
        next_message(&mut ws).await; // PairChallenge, code discarded on purpose

        let confirm = Message::PairConfirm {
            code: "000000".to_string(),
        };
        ws.send(WsMessage::Text(
            serde_json::to_string(&confirm).unwrap().into(),
        ))
        .await
        .expect("send PairConfirm");

        let reply = next_message(&mut ws).await;
        let expected_code = if attempt < MAX_ATTEMPTS {
            "PAIRING_FAILED"
        } else {
            "TOO_MANY_ATTEMPTS"
        };
        match reply {
            Message::Error { code, .. } => assert_eq!(code, expected_code),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // The server should have closed the connection after locking.
    let next = ws.next().await;
    match next {
        None => {}
        Some(Ok(WsMessage::Close(_))) => {}
        other => panic!("expected the connection to be closed, got {other:?}"),
    }
}

async fn next_message(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Message {
    loop {
        let frame = ws
            .next()
            .await
            .expect("connection closed before a message arrived")
            .expect("websocket error");
        if let WsMessage::Text(text) = frame {
            return serde_json::from_str(&text).expect("deserialize protocol::Message");
        }
    }
}
