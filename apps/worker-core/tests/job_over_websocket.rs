//! Pairs, then submits a real JobRequest over a real WebSocket connection
//! and verifies the full Text+Binary message sequence, including
//! reassembling the chunked artifact and checking its SHA-256.

use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use protocol::Message;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use worker_core::{AppState, PeerStore};

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

async fn spawn_test_server() -> SocketAddr {
    let peer_store = PeerStore::load(std::env::temp_dir().join(format!(
        "distbuild-test-peers-{}-job-ws.json",
        std::process::id()
    )))
    .expect("load peer store");

    let state = AppState {
        worker_id: "worker-test-1".to_string(),
        worker_name: "Test-Worker".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
        runtime_dir: std::env::temp_dir().join(format!(
            "distbuild-test-runtime-{}-job-ws",
            std::process::id()
        )),
        jobs: Arc::new(Mutex::new(std::collections::HashMap::new())),
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

#[tokio::test]
async fn submits_a_real_job_and_receives_a_reassembled_artifact() {
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let addr = spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");

    // pair
    ws.send(WsMessage::Text(
        serde_json::to_string(&Message::PairRequest {
            master_name: "MacBook-Pro".to_string(),
            master_id: "master-job-test".to_string(),
            reconnect_token: None,
        })
        .unwrap()
        .into(),
    ))
    .await
    .expect("send");
    let code = match next_message(&mut ws).await {
        Message::PairChallenge {
            code_shown_on_worker,
        } => code_shown_on_worker,
        other => panic!("expected PairChallenge, got {other:?}"),
    };
    ws.send(WsMessage::Text(
        serde_json::to_string(&Message::PairConfirm { code })
            .unwrap()
            .into(),
    ))
    .await
    .expect("send");
    next_message(&mut ws).await; // PairAccepted

    // submit
    ws.send(WsMessage::Text(
        serde_json::to_string(&Message::JobRequest {
            job_id: "job-ws-1".to_string(),
            repo: src.path().to_string_lossy().into_owned(),
            branch: "main".to_string(),
            profile: "debug".to_string(),
            env: None,
            distbuild_toml: None,
        })
        .unwrap()
        .into(),
    ))
    .await
    .expect("send");

    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut expected_sha256 = String::new();
    let mut finished = false;

    while !finished {
        let frame = ws
            .next()
            .await
            .expect("connection closed early")
            .expect("ws error");
        match frame {
            WsMessage::Text(text) => {
                let message: Message = serde_json::from_str(&text).expect("deserialize");
                match message {
                    Message::ArtifactReady { sha256, .. } => expected_sha256 = sha256,
                    Message::JobFinished { success, .. } => {
                        assert!(success, "fixture build should succeed");
                        finished = true;
                    }
                    _ => {}
                }
            }
            WsMessage::Binary(data) => {
                let header_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
                let header: artifacts::ChunkHeader =
                    serde_json::from_slice(&data[4..4 + header_len]).expect("header");
                chunks.push((header.chunk_index, data[4 + header_len..].to_vec()));
            }
            _ => {}
        }
    }

    assert!(
        !chunks.is_empty(),
        "artifact should be sent in at least one chunk"
    );
    chunks.sort_by_key(|(index, _)| *index);
    let reassembled: Vec<u8> = chunks.into_iter().flat_map(|(_, bytes)| bytes).collect();
    let mut hasher = Sha256::new();
    hasher.update(&reassembled);
    assert_eq!(hex::encode(hasher.finalize()), expected_sha256);
}
