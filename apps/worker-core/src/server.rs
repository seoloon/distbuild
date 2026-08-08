use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::sync::Mutex;

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

    while let Some(Ok(msg)) = socket.recv().await {
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
}
