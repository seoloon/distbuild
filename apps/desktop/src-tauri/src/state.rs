use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::peers::MasterPeerStore;
use crate::ws_client::PairingSession;

#[derive(Clone)]
pub struct AppState {
    pub runtime_dir: PathBuf,
    pub master_id: String,
    pub master_name: String,
    pub peer_store: Arc<Mutex<MasterPeerStore>>,
    /// Cancellation handles for jobs this Master has submitted, keyed by
    /// job_id, so `cancel_job` can signal the streaming task without
    /// tearing down the whole connection loop from outside it.
    pub active_jobs: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Pairing connections between `pair_worker` (opens it, shows the
    /// code) and `confirm_pair` (sends the human-confirmed code on it),
    /// keyed by the dialed `(host, port)`.
    pub pending_pairings: Arc<Mutex<HashMap<(String, u16), PairingSession>>>,
}

pub fn build_app_state() -> AppState {
    let runtime_dir = dirs::document_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("DistBuild");

    let peer_store = MasterPeerStore::load(runtime_dir.join("peers-master.json"))
        .expect("failed to load peers-master.json");

    AppState {
        master_id: load_or_generate_master_id(&runtime_dir),
        master_name: master_hostname(),
        peer_store: Arc::new(Mutex::new(peer_store)),
        active_jobs: Arc::new(Mutex::new(HashMap::new())),
        pending_pairings: Arc::new(Mutex::new(HashMap::new())),
        runtime_dir,
    }
}

fn master_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "distbuild-master".to_string())
}

/// Generates this Master's identity once and persists it, since workers
/// key their "persistent trust" (PROMPT.md's security section) by
/// `master_id` — a fresh random ID on every launch would force re-pairing
/// every time the app restarts.
fn load_or_generate_master_id(runtime_dir: &Path) -> String {
    let path = runtime_dir.join("master_id.txt");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = format!("master-{:016x}", rand::random::<u64>());
    let _ = std::fs::create_dir_all(runtime_dir);
    let _ = std::fs::write(&path, &id);
    id
}
