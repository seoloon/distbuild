use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

/// A worker this Master has successfully paired with, including the
/// SHA-256 fingerprint of its TLS certificate — pinned per PROMPT.md's
/// security section so every connection after pairing can be verified
/// against exactly this worker, not just any self-signed certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterPeer {
    pub worker_id: String,
    pub worker_name: String,
    pub host: String,
    pub port: u16,
    #[serde(with = "hex_fingerprint")]
    pub fingerprint: [u8; 32],
    /// Current secret this Master presents on reconnect to skip the
    /// human-confirmed code — see
    /// `worker_core::pairing::PairingSession::accept_as_already_paired`.
    /// Rotated every time it's used.
    pub reconnect_token: String,
}

mod hex_fingerprint {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(fingerprint: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(fingerprint))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("fingerprint must be exactly 32 bytes"))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PeersFile {
    peers: Vec<MasterPeer>,
}

/// Persistent record of paired workers, stored at `peers-master.json`
/// (distinct from `worker-core`'s own `peers.json` — the two roles trust
/// different, differently-shaped peer records even though they share one
/// binary and runtime directory).
#[derive(Debug)]
pub struct MasterPeerStore {
    path: PathBuf,
    peers: HashMap<String, MasterPeer>,
}

impl MasterPeerStore {
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
            .map(|p| (p.worker_id.clone(), p))
            .collect();
        Ok(Self { path, peers })
    }

    pub fn is_paired(&self, worker_id: &str) -> bool {
        self.peers.contains_key(worker_id)
    }

    pub fn get(&self, worker_id: &str) -> Option<&MasterPeer> {
        self.peers.get(worker_id)
    }

    pub fn add_and_save(&mut self, peer: MasterPeer) -> Result<(), PeerStoreError> {
        self.peers.insert(peer.worker_id.clone(), peer);
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
        write_owner_only(&self.path, &json).map_err(|source| PeerStoreError::Write {
            path: self.path.clone(),
            source,
        })
    }
}

/// Writes `contents` with owner-only permissions set at creation time
/// rather than written-then-chmod'd — this file holds reconnect tokens,
/// which are bearer secrets. See `worker_core::pairing`'s identical
/// pattern (applied there for the same reason) and Phase 2's TLS
/// private-key fix this mirrors.
#[cfg(unix)]
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_store_roundtrips_through_disk_including_the_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("peers-master.json");

        let mut store = MasterPeerStore::load(&path).expect("load empty store");
        assert!(!store.is_paired("worker-1"));

        let fingerprint = [7u8; 32];
        store
            .add_and_save(MasterPeer {
                worker_id: "worker-1".to_string(),
                worker_name: "Threadripper-Box".to_string(),
                host: "192.168.1.42".to_string(),
                port: 7878,
                fingerprint,
                reconnect_token: "a".repeat(64),
            })
            .expect("add_and_save");

        let reloaded = MasterPeerStore::load(&path).expect("reload");
        let peer = reloaded.get("worker-1").expect("peer present");
        assert_eq!(peer.fingerprint, fingerprint);
        assert_eq!(peer.host, "192.168.1.42");
        assert_eq!(peer.reconnect_token, "a".repeat(64));
    }

    #[cfg(unix)]
    #[test]
    fn peers_file_is_restricted_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("peers-master.json");
        let mut store = MasterPeerStore::load(&path).expect("load empty store");
        store
            .add_and_save(MasterPeer {
                worker_id: "worker-1".to_string(),
                worker_name: "Threadripper-Box".to_string(),
                host: "192.168.1.42".to_string(),
                port: 7878,
                fingerprint: [7u8; 32],
                reconnect_token: "a".repeat(64),
            })
            .expect("add_and_save");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
