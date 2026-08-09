use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use protocol::Message;
use rustls::ClientConfig;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{
    connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

use crate::peers::MasterPeer;
use crate::tls_verify::{provider, PinnedVerifier, TofuVerifier};

pub type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Error)]
pub enum WsClientError {
    #[error("failed to connect to {host}:{port}: {source}")]
    Connect {
        host: String,
        port: u16,
        #[source]
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[error("connection closed before a reply arrived")]
    ClosedEarly,
    #[error("unexpected reply: {0:?}")]
    UnexpectedReply(Message),
    #[error("worker rejected pairing: {code} — {message}")]
    Rejected { code: String, message: String },
}

/// State held between `begin_pairing` (shows the code) and `confirm_pairing`
/// (sends the human-confirmed code back) — the same open connection is
/// reused across both steps.
pub struct PairingSession {
    pub code: String,
    pub fingerprint: [u8; 32],
    stream: WsStream,
}

async fn connect_with_verifier(
    host: &str,
    port: u16,
    verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
) -> Result<WsStream, WsClientError> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    let url = format!("wss://{host}:{port}/ws");
    let (stream, _response) =
        connect_async_tls_with_config(&url, None, false, Some(Connector::Rustls(Arc::new(config))))
            .await
            .map_err(|source| WsClientError::Connect {
                host: host.to_string(),
                port,
                source,
            })?;
    Ok(stream)
}

async fn send(stream: &mut WsStream, message: &Message) -> Result<(), WsClientError> {
    let json = serde_json::to_string(message).expect("Message serialization cannot fail");
    stream
        .send(WsMessage::Text(json.into()))
        .await
        .map_err(|source| WsClientError::Connect {
            host: String::new(),
            port: 0,
            source,
        })
}

async fn recv_message(stream: &mut WsStream) -> Result<Message, WsClientError> {
    loop {
        let frame = stream.next().await.ok_or(WsClientError::ClosedEarly)?;
        let frame = frame.map_err(|source| WsClientError::Connect {
            host: String::new(),
            port: 0,
            source,
        })?;
        if let WsMessage::Text(text) = frame {
            return serde_json::from_str(&text).map_err(|_| WsClientError::ClosedEarly);
        }
    }
}

/// Opens a TOFU connection, sends `PairRequest`, and returns the
/// challenge code (for display) plus the captured certificate fingerprint
/// (for pinning once the human confirms).
pub async fn begin_pairing(
    host: &str,
    port: u16,
    master_name: &str,
    master_id: &str,
) -> Result<PairingSession, WsClientError> {
    let tofu = TofuVerifier::new(provider());
    let mut stream = connect_with_verifier(host, port, tofu.clone()).await?;

    send(
        &mut stream,
        &Message::PairRequest {
            master_name: master_name.to_string(),
            master_id: master_id.to_string(),
            reconnect_token: None,
        },
    )
    .await?;

    let reply = recv_message(&mut stream).await?;
    let code = match reply {
        Message::PairChallenge {
            code_shown_on_worker,
        } => code_shown_on_worker,
        Message::Error { code, message } => return Err(WsClientError::Rejected { code, message }),
        other => return Err(WsClientError::UnexpectedReply(other)),
    };

    let fingerprint = tofu
        .captured_fingerprint()
        .expect("verifier is called before any reply arrives");
    Ok(PairingSession {
        code,
        fingerprint,
        stream,
    })
}

/// Sends the human-confirmed code on the still-open pairing connection and
/// returns the resulting `MasterPeer` to persist. `host`/`port` are filled
/// in by the caller (`commands.rs`), which knows the dialed address — this
/// function only speaks the protocol over an already-verified connection.
pub async fn confirm_pairing(
    mut session: PairingSession,
    code: String,
) -> Result<MasterPeer, WsClientError> {
    send(&mut session.stream, &Message::PairConfirm { code }).await?;
    let reply = recv_message(&mut session.stream).await?;
    match reply {
        Message::PairAccepted {
            worker_id,
            worker_name,
            reconnect_token,
            ..
        } => Ok(MasterPeer {
            worker_id,
            worker_name,
            host: String::new(),
            port: 0,
            fingerprint: session.fingerprint,
            reconnect_token,
        }),
        Message::Error { code, message } => Err(WsClientError::Rejected { code, message }),
        other => Err(WsClientError::UnexpectedReply(other)),
    }
}

/// Opens a connection to an already-paired worker, pinned to its stored
/// certificate fingerprint. Rejects the handshake outright if the
/// presented certificate doesn't match.
pub async fn connect_paired(peer: &MasterPeer) -> Result<WsStream, WsClientError> {
    let verifier = PinnedVerifier::new(provider(), peer.fingerprint);
    connect_with_verifier(&peer.host, peer.port, verifier).await
}

/// Opens a pinned connection to an already-paired worker and completes
/// the pairing handshake on it. Pairing state lives per-connection on the
/// worker, not per-identity, so every new connection needs *a*
/// `PairRequest` — but presenting `peer.reconnect_token` (a rotating
/// bearer secret from the previous pairing, *not* the bare `master_id`,
/// which is not a secret) lets the worker accept it immediately without a
/// fresh human-confirmed code (see
/// `worker_core::pairing::PairingSession::accept_as_already_paired`).
///
/// Returns the ready-to-use stream plus the freshly rotated token — the
/// caller (`commands.rs`) must persist it to `MasterPeerStore`, since the
/// one just used is now invalid for future reconnects.
pub async fn resume_paired_connection(
    peer: &MasterPeer,
    master_name: &str,
    master_id: &str,
) -> Result<(WsStream, String), WsClientError> {
    let mut stream = connect_paired(peer).await?;
    send(
        &mut stream,
        &Message::PairRequest {
            master_name: master_name.to_string(),
            master_id: master_id.to_string(),
            reconnect_token: Some(peer.reconnect_token.clone()),
        },
    )
    .await?;

    let reply = recv_message(&mut stream).await?;
    match reply {
        Message::PairAccepted {
            reconnect_token, ..
        } => Ok((stream, reconnect_token)),
        Message::Error { code, message } => Err(WsClientError::Rejected { code, message }),
        other => Err(WsClientError::UnexpectedReply(other)),
    }
}

pub struct JobRequestArgs {
    pub job_id: String,
    pub repo: String,
    pub branch: String,
    pub profile: String,
}

/// Where `run_job_stream` forwards protocol updates for the frontend to
/// observe. Kept independent of `tauri::AppHandle` so the job-streaming
/// logic can be exercised in tests without spinning up a Tauri runtime
/// (constructing a real `AppHandle`, even `tauri::test`'s mock one,
/// pulls in native webview runtime initialization that has no bearing on
/// this function's actual logic).
pub trait JobEventSink: Send + 'static {
    fn emit(&self, event: &str, message: &Message);
}

impl<R: tauri::Runtime> JobEventSink for tauri::AppHandle<R> {
    fn emit(&self, event: &str, message: &Message) {
        let _ = tauri::Emitter::emit(self, event, message);
    }
}

/// Sends `JobRequest` on an already-connected, pinned socket, then relays
/// every reply to `sink` as an event (`job://log`, `job://progress`,
/// `job://artifact`, `job://finished`) until the job finishes or the
/// connection drops. Binary chunk frames (framed per
/// `worker_core::jobs`'s doc comment: `[u32 LE header_len][header
/// JSON][chunk bytes]`) are reassembled and written to
/// `<artifacts_dir>/<job_id>/<filename>` once the last chunk arrives.
///
/// `cancel` is watched cooperatively: once cancelled, a single
/// `JobCancel` is sent and this function keeps reading until the worker
/// reports `JobFinished` (with `success: false`) or the connection ends.
pub async fn run_job_stream<S: JobEventSink>(
    mut stream: WsStream,
    job: JobRequestArgs,
    sink: S,
    artifacts_dir: PathBuf,
    cancel: CancellationToken,
) -> Result<(), WsClientError> {
    let job_id = job.job_id.clone();

    send(
        &mut stream,
        &Message::JobRequest {
            job_id: job.job_id.clone(),
            repo: job.repo,
            branch: job.branch,
            profile: job.profile,
            env: None,
            distbuild_toml: None,
        },
    )
    .await?;

    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut artifact_filename: Option<String> = None;
    let mut cancel_sent = false;

    loop {
        let frame = if cancel_sent {
            stream.next().await
        } else {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = send(&mut stream, &Message::JobCancel { job_id: job_id.clone() }).await;
                    cancel_sent = true;
                    continue;
                }
                frame = stream.next() => frame,
            }
        };

        let Some(frame) = frame else { break };
        let frame = frame.map_err(|source| WsClientError::Connect {
            host: String::new(),
            port: 0,
            source,
        })?;

        match frame {
            WsMessage::Text(text) => {
                let message: Message = match serde_json::from_str(&text) {
                    Ok(message) => message,
                    Err(_) => continue,
                };
                match &message {
                    Message::LogChunk { .. } => {
                        sink.emit("job://log", &message);
                    }
                    Message::JobProgress { .. } => {
                        sink.emit("job://progress", &message);
                    }
                    Message::ArtifactReady { filename, .. } => {
                        artifact_filename = Some(filename.clone());
                        sink.emit("job://artifact", &message);
                    }
                    Message::JobFinished { .. } => {
                        sink.emit("job://finished", &message);
                        break;
                    }
                    _ => {}
                }
            }
            WsMessage::Binary(data) => {
                if data.len() < 4 {
                    continue;
                }
                let header_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
                if data.len() < 4 + header_len {
                    continue;
                }
                let Ok(header) =
                    serde_json::from_slice::<artifacts::ChunkHeader>(&data[4..4 + header_len])
                else {
                    continue;
                };
                chunks.push((header.chunk_index, data[4 + header_len..].to_vec()));
            }
            _ => {}
        }
    }

    if let Some(filename) = artifact_filename {
        if !chunks.is_empty() {
            // `filename` came from the worker over the wire — never trust
            // it as a path directly. Keep only its final path component so
            // a malicious or buggy worker can't write outside `job_dir`
            // via a `../`-laden filename.
            if let Some(safe_name) = std::path::Path::new(&filename).file_name() {
                chunks.sort_by_key(|(index, _)| *index);
                let reassembled: Vec<u8> =
                    chunks.into_iter().flat_map(|(_, bytes)| bytes).collect();
                let job_dir = artifacts_dir.join(&job_id);
                if std::fs::create_dir_all(&job_dir).is_ok() {
                    let _ = std::fs::write(job_dir.join(safe_name), &reassembled);
                }
            }
        }
    }

    Ok(())
}
