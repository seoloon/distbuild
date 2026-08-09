use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::sync::Mutex;

use crate::jobs::{run_job, JobEvent, JobHandle, JobParams};
use crate::net::is_allowed_addr;
use crate::pairing::{PairingSession, PeerStore};

/// Shared state for the worker's `/ws` endpoint.
#[derive(Clone)]
pub struct AppState {
    pub worker_id: String,
    pub worker_name: String,
    pub os: String,
    pub arch: String,
    pub peer_store: Arc<Mutex<PeerStore>>,
    /// Base directory jobs are cloned/built/packaged under (`<runtime_dir>/jobs/<job_id>/...`).
    pub runtime_dir: PathBuf,
    /// Cancellation handles for jobs currently running on any connection,
    /// keyed by job_id, so a `Message::JobCancel` can be routed to the
    /// right in-flight `run_job` task.
    pub jobs: Arc<Mutex<HashMap<String, JobHandle>>>,
}

/// Builds the worker's Axum router. Transport-agnostic: TLS termination
/// happens at the `axum_server::tls_rustls::bind_rustls` layer in `main`,
/// not here — this router works identically over plain or TLS-terminated
/// connections, which is what makes it testable without a certificate.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if !is_allowed_addr(addr.ip()) {
        tracing::warn!(%addr, "rejected /ws upgrade from disallowed address");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut session = PairingSession::new(
        state.worker_id.clone(),
        state.worker_name.clone(),
        state.os.clone(),
        state.arch.clone(),
    );

    let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel::<JobEvent>();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    WsMessage::Text(text) => text,
                    WsMessage::Close(_) => break,
                    _ => continue,
                };

                let incoming: protocol::Message = match serde_json::from_str(text.as_str()) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!(%error, "failed to parse incoming message");
                        continue;
                    }
                };

                if !session.is_paired() {
                    if let protocol::Message::PairRequest { master_id, reconnect_token: Some(token), .. } = &incoming {
                        // master_id alone is a bare, non-secret identifier — only a
                        // matching reconnect_token (a rotating bearer secret handed
                        // out on the previous pairing) authorizes skipping the
                        // human-confirmed code. Any mismatch or unknown master_id
                        // falls through to the normal challenge/confirm flow below,
                        // never to an error — a forged token just means "pair like normal".
                        let known_peer = state.peer_store.lock().await.get(master_id).cloned();
                        if let Some(peer) = known_peer {
                            if peer.reconnect_token == *token {
                                let (reply, rotated_peer) =
                                    session.accept_as_already_paired(master_id, &peer.master_name);
                                if let Err(error) =
                                    state.peer_store.lock().await.add_and_save(rotated_peer)
                                {
                                    tracing::error!(%error, "failed to persist rotated reconnect token");
                                }
                                let json = serde_json::to_string(&reply)
                                    .expect("Message serialization cannot fail");
                                if socket.send(WsMessage::Text(json.into())).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        }
                    }
                }

                if session.is_paired() {
                    match &incoming {
                        protocol::Message::JobRequest { job_id, repo, branch, profile, .. } => {
                            let handle = JobHandle::new();
                            state.jobs.lock().await.insert(job_id.clone(), handle.clone());

                            let params = JobParams {
                                job_id: job_id.clone(),
                                repo: repo.clone(),
                                branch: branch.clone(),
                                profile: profile.clone(),
                            };
                            let runtime_dir = state.runtime_dir.clone();
                            let job_tx = job_tx.clone();
                            tokio::spawn(async move {
                                if let Err(error) = run_job(params, &runtime_dir, job_tx, handle).await {
                                    tracing::error!(%error, "job dispatcher failed");
                                }
                            });
                            continue;
                        }
                        protocol::Message::JobCancel { job_id } => {
                            if let Some(handle) = state.jobs.lock().await.get(job_id) {
                                handle.cancel();
                            }
                            continue;
                        }
                        _ => {}
                    }
                }

                let (reply, new_peer) = session.handle(&incoming);

                if let Some(peer) = new_peer {
                    let mut store = state.peer_store.lock().await;
                    if let Err(error) = store.add_and_save(peer) {
                        tracing::error!(%error, "failed to persist paired peer");
                    }
                }

                if let Some(reply) = reply {
                    let json = serde_json::to_string(&reply).expect("Message serialization cannot fail");
                    if socket.send(WsMessage::Text(json.into())).await.is_err() {
                        break;
                    }
                }

                if session.is_locked() {
                    tracing::warn!("too many failed pairing attempts on this connection; closing it");
                    let _ = socket.send(WsMessage::Close(None)).await;
                    break;
                }
            }
            Some(event) = job_rx.recv() => {
                let send_result = match event {
                    JobEvent::Text(message) => {
                        let json = serde_json::to_string(&message).expect("Message serialization cannot fail");
                        socket.send(WsMessage::Text(json.into())).await
                    }
                    JobEvent::Binary(bytes) => socket.send(WsMessage::Binary(bytes.into())).await,
                };
                if send_result.is_err() {
                    break;
                }
            }
        }
    }
}
