use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use protocol::Message;
use rustls::ClientConfig;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{
    connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream,
};

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
            ..
        } => Ok(MasterPeer {
            worker_id,
            worker_name,
            host: String::new(),
            port: 0,
            fingerprint: session.fingerprint,
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
