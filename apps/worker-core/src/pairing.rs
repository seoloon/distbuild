use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use protocol::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PeerStoreError {
    #[error("failed to read peers file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write peers file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse peers file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// A Master this worker has successfully paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub master_id: String,
    pub master_name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PeersFile {
    peers: Vec<Peer>,
}

/// Persistent record of paired masters, stored at `peers.json` per
/// PROMPT.md's runtime directory layout ("persistent trust").
#[derive(Debug)]
pub struct PeerStore {
    path: PathBuf,
    peers: HashMap<String, Peer>,
}

impl PeerStore {
    /// Loads `path` if it exists, or starts an empty store (the file is
    /// created lazily on the first successful pairing).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, PeerStoreError> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self {
                path,
                peers: HashMap::new(),
            });
        }
        let contents = fs::read_to_string(&path).map_err(|source| PeerStoreError::Read {
            path: path.clone(),
            source,
        })?;
        let file: PeersFile =
            serde_json::from_str(&contents).map_err(|source| PeerStoreError::Parse {
                path: path.clone(),
                source,
            })?;
        let peers = file
            .peers
            .into_iter()
            .map(|p| (p.master_id.clone(), p))
            .collect();
        Ok(Self { path, peers })
    }

    pub fn is_paired(&self, master_id: &str) -> bool {
        self.peers.contains_key(master_id)
    }

    pub fn add_and_save(&mut self, peer: Peer) -> Result<(), PeerStoreError> {
        self.peers.insert(peer.master_id.clone(), peer);
        self.save()
    }

    fn save(&self) -> Result<(), PeerStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| PeerStoreError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let file = PeersFile {
            peers: self.peers.values().cloned().collect(),
        };
        let json =
            serde_json::to_string_pretty(&file).expect("PeersFile serialization cannot fail");
        fs::write(&self.path, json).map_err(|source| PeerStoreError::Write {
            path: self.path.clone(),
            source,
        })
    }
}

/// After this many failed `PairConfirm` attempts on a single connection,
/// the session locks permanently and the connection must be dropped and
/// re-established to try again — bounding brute-force guessing of the
/// 6-digit code to a handful of attempts per TCP/WS handshake.
const MAX_PAIRING_ATTEMPTS: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairingStatus {
    AwaitingRequest,
    AwaitingConfirm,
    Paired,
    Locked,
}

/// Drives one connection's challenge/confirm pairing flow, per
/// PROMPT.md's protocol section. Pure state machine — no I/O, no
/// networking — so it can be tested without a running server.
pub struct PairingSession {
    worker_id: String,
    worker_name: String,
    os: String,
    arch: String,
    status: PairingStatus,
    expected_code: Option<String>,
    pending_master: Option<(String, String)>,
    failed_attempts: u8,
}

impl PairingSession {
    pub fn new(
        worker_id: impl Into<String>,
        worker_name: impl Into<String>,
        os: impl Into<String>,
        arch: impl Into<String>,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            worker_name: worker_name.into(),
            os: os.into(),
            arch: arch.into(),
            status: PairingStatus::AwaitingRequest,
            expected_code: None,
            pending_master: None,
            failed_attempts: 0,
        }
    }

    pub fn is_paired(&self) -> bool {
        self.status == PairingStatus::Paired
    }

    /// True once too many wrong codes have been submitted on this
    /// connection. The caller (the `/ws` handler) should send the final
    /// reply and then close the socket rather than keep reading.
    pub fn is_locked(&self) -> bool {
        self.status == PairingStatus::Locked
    }

    /// Handles one incoming [`Message`], returning the reply to send back
    /// (if any) and, on newly-completed pairing, the [`Peer`] the caller
    /// should persist via [`PeerStore::add_and_save`].
    pub fn handle(&mut self, message: &Message) -> (Option<Message>, Option<Peer>) {
        match (self.status, message) {
            (
                PairingStatus::AwaitingRequest,
                Message::PairRequest {
                    master_name,
                    master_id,
                },
            ) => {
                let code = generate_code();
                // Deliberately not logged: the code is a shared secret meant
                // to travel only through the PairChallenge reply (for the
                // caller to display locally) and the human's own eyes — not
                // through log aggregation.
                tracing::info!(
                    master_name = %master_name,
                    "pairing requested — a pairing code has been generated for display on the worker"
                );
                self.pending_master = Some((master_id.clone(), master_name.clone()));
                self.expected_code = Some(code.clone());
                self.status = PairingStatus::AwaitingConfirm;
                (
                    Some(Message::PairChallenge {
                        code_shown_on_worker: code,
                    }),
                    None,
                )
            }
            (PairingStatus::AwaitingConfirm, Message::PairConfirm { code }) => {
                let expected = self.expected_code.take();
                if expected.as_deref() == Some(code.trim()) {
                    self.status = PairingStatus::Paired;
                    let (master_id, master_name) = self
                        .pending_master
                        .clone()
                        .expect("pending_master is set alongside expected_code");
                    let peer = Peer {
                        master_id,
                        master_name,
                    };
                    (
                        Some(Message::PairAccepted {
                            worker_id: self.worker_id.clone(),
                            worker_name: self.worker_name.clone(),
                            os: self.os.clone(),
                            arch: self.arch.clone(),
                        }),
                        Some(peer),
                    )
                } else {
                    self.failed_attempts += 1;
                    if self.failed_attempts >= MAX_PAIRING_ATTEMPTS {
                        self.status = PairingStatus::Locked;
                        (
                            Some(Message::Error {
                                code: "TOO_MANY_ATTEMPTS".to_string(),
                                message: "too many failed pairing attempts; reconnect to try again"
                                    .to_string(),
                            }),
                            None,
                        )
                    } else {
                        // A fresh PairRequest (and thus a fresh code) is
                        // required to retry — the stale confirm no longer
                        // matches anything.
                        self.status = PairingStatus::AwaitingRequest;
                        (
                            Some(Message::Error {
                                code: "PAIRING_FAILED".to_string(),
                                message: "pairing code did not match".to_string(),
                            }),
                            None,
                        )
                    }
                }
            }
            (PairingStatus::Paired, _) => (
                Some(Message::Error {
                    code: "ALREADY_PAIRED".to_string(),
                    message: "this connection is already paired".to_string(),
                }),
                None,
            ),
            (PairingStatus::Locked, _) => (
                Some(Message::Error {
                    code: "TOO_MANY_ATTEMPTS".to_string(),
                    message: "too many failed pairing attempts; reconnect to try again".to_string(),
                }),
                None,
            ),
            _ => (
                Some(Message::Error {
                    code: "UNEXPECTED_MESSAGE".to_string(),
                    message: "message is not valid for the current pairing state".to_string(),
                }),
                None,
            ),
        }
    }
}

fn generate_code() -> String {
    format!("{:06}", rand::random_range(0..1_000_000u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair_request() -> Message {
        Message::PairRequest {
            master_name: "MacBook-Pro".to_string(),
            master_id: "master-abc123".to_string(),
        }
    }

    #[test]
    fn successful_pairing_roundtrip() {
        let mut session = PairingSession::new("worker-1", "Threadripper-Box", "linux", "x86_64");

        let (reply, peer) = session.handle(&pair_request());
        let code = match reply {
            Some(Message::PairChallenge {
                code_shown_on_worker,
            }) => code_shown_on_worker,
            other => panic!("expected PairChallenge, got {other:?}"),
        };
        assert!(peer.is_none());
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(!session.is_paired());

        let (reply, peer) = session.handle(&Message::PairConfirm { code });
        assert!(session.is_paired());
        let peer = peer.expect("pairing should produce a Peer to persist");
        assert_eq!(peer.master_id, "master-abc123");
        assert_eq!(peer.master_name, "MacBook-Pro");
        match reply {
            Some(Message::PairAccepted {
                worker_id,
                worker_name,
                os,
                arch,
            }) => {
                assert_eq!(worker_id, "worker-1");
                assert_eq!(worker_name, "Threadripper-Box");
                assert_eq!(os, "linux");
                assert_eq!(arch, "x86_64");
            }
            other => panic!("expected PairAccepted, got {other:?}"),
        }
    }

    #[test]
    fn wrong_code_is_rejected_and_resets_the_session() {
        let mut session = PairingSession::new("worker-1", "Threadripper-Box", "linux", "x86_64");
        session.handle(&pair_request());

        let (reply, peer) = session.handle(&Message::PairConfirm {
            code: "000000".to_string(),
        });
        assert!(peer.is_none());
        assert!(!session.is_paired());
        match reply {
            Some(Message::Error { code, .. }) => assert_eq!(code, "PAIRING_FAILED"),
            other => panic!("expected Error, got {other:?}"),
        }

        // A fresh PairRequest is required to retry — the stale confirm no longer works.
        let (reply, _) = session.handle(&Message::PairConfirm {
            code: "000000".to_string(),
        });
        match reply {
            Some(Message::Error { code, .. }) => assert_eq!(code, "UNEXPECTED_MESSAGE"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn locks_after_max_failed_attempts_and_rejects_everything_afterward() {
        let mut session = PairingSession::new("worker-1", "Threadripper-Box", "linux", "x86_64");

        for attempt in 1..=MAX_PAIRING_ATTEMPTS {
            session.handle(&pair_request());
            let (reply, peer) = session.handle(&Message::PairConfirm {
                code: "000000".to_string(),
            });
            assert!(peer.is_none());
            if attempt < MAX_PAIRING_ATTEMPTS {
                assert!(!session.is_locked(), "should not lock before the limit");
                match reply {
                    Some(Message::Error { code, .. }) => assert_eq!(code, "PAIRING_FAILED"),
                    other => panic!("expected Error, got {other:?}"),
                }
            } else {
                assert!(session.is_locked(), "should lock at the limit");
                match reply {
                    Some(Message::Error { code, .. }) => assert_eq!(code, "TOO_MANY_ATTEMPTS"),
                    other => panic!("expected Error, got {other:?}"),
                }
            }
        }

        // Once locked, even a well-formed, freshly-requested pairing is refused.
        let (reply, peer) = session.handle(&pair_request());
        assert!(peer.is_none());
        match reply {
            Some(Message::Error { code, .. }) => assert_eq!(code, "TOO_MANY_ATTEMPTS"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn confirm_before_request_is_rejected() {
        let mut session = PairingSession::new("worker-1", "Threadripper-Box", "linux", "x86_64");
        let (reply, peer) = session.handle(&Message::PairConfirm {
            code: "123456".to_string(),
        });
        assert!(peer.is_none());
        match reply {
            Some(Message::Error { code, .. }) => assert_eq!(code, "UNEXPECTED_MESSAGE"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn messages_after_pairing_are_rejected() {
        let mut session = PairingSession::new("worker-1", "Threadripper-Box", "linux", "x86_64");
        let (reply, _) = session.handle(&pair_request());
        let code = match reply {
            Some(Message::PairChallenge {
                code_shown_on_worker,
            }) => code_shown_on_worker,
            other => panic!("expected PairChallenge, got {other:?}"),
        };
        session.handle(&Message::PairConfirm { code });
        assert!(session.is_paired());

        let (reply, _) = session.handle(&pair_request());
        match reply {
            Some(Message::Error { code, .. }) => assert_eq!(code, "ALREADY_PAIRED"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn peer_store_roundtrips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("peers.json");

        let mut store = PeerStore::load(&path).expect("load empty store");
        assert!(!store.is_paired("master-abc123"));

        store
            .add_and_save(Peer {
                master_id: "master-abc123".to_string(),
                master_name: "MacBook-Pro".to_string(),
            })
            .expect("add_and_save");
        assert!(store.is_paired("master-abc123"));

        let reloaded = PeerStore::load(&path).expect("reload store");
        assert!(reloaded.is_paired("master-abc123"));
    }
}
