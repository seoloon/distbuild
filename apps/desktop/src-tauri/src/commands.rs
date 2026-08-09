use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::peers::MasterPeer;
use crate::state::AppState;
use crate::{tls_verify, ws_client};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoveredWorkerDto {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub port: u16,
    pub version: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct PairingChallenge {
    pub host: String,
    pub port: u16,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct PairedWorkerDto {
    pub worker_id: String,
    pub worker_name: String,
}

/// Browses for `_distbuild._tcp.local.` workers for a fixed short window
/// and returns a snapshot of what was found. Runs the blocking mDNS
/// receive loop on the blocking thread pool per PROMPT.md's rule against
/// blocking IO in async handlers.
#[tauri::command]
pub async fn discover_workers() -> Result<Vec<DiscoveredWorkerDto>, String> {
    tokio::task::spawn_blocking(|| {
        let daemon = discovery::ServiceDaemon::new().map_err(|e| e.to_string())?;
        let receiver = discovery::browse(&daemon).map_err(|e| e.to_string())?;

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut workers = Vec::new();

        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(discovery::ServiceEvent::ServiceResolved(resolved)) => {
                    if let Some(worker) = discovery::parse_worker(&resolved) {
                        workers.push(DiscoveredWorkerDto {
                            name: worker.name,
                            os: worker.os,
                            arch: worker.arch,
                            port: worker.port,
                            version: worker.version,
                            addresses: worker.addresses.iter().map(|a| a.to_string()).collect(),
                        });
                    }
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }

        let _ = daemon.shutdown();
        Ok(workers)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Step 1 of pairing: connects TOFU, sends `PairRequest`, and returns the
/// challenge code to display. The still-open connection is parked in
/// `AppState::pending_pairings` (keyed by `host:port`) for `confirm_pair`
/// to resume.
#[tauri::command]
pub async fn pair_worker(
    state: State<'_, AppState>,
    host: String,
    port: u16,
) -> Result<PairingChallenge, String> {
    tls_verify::install_crypto_provider();
    let session = ws_client::begin_pairing(&host, port, &state.master_name, &state.master_id)
        .await
        .map_err(|e| e.to_string())?;
    let code = session.code.clone();

    state
        .pending_pairings
        .lock()
        .await
        .insert((host.clone(), port), session);

    Ok(PairingChallenge { host, port, code })
}

/// Step 2 of pairing: sends the human-confirmed code on the connection
/// `pair_worker` opened, and persists the resulting peer (with its pinned
/// certificate fingerprint) on success.
#[tauri::command]
pub async fn confirm_pair(
    state: State<'_, AppState>,
    host: String,
    port: u16,
    code: String,
) -> Result<PairedWorkerDto, String> {
    let session = state
        .pending_pairings
        .lock()
        .await
        .remove(&(host.clone(), port))
        .ok_or_else(|| "no pairing in progress for this worker".to_string())?;

    let mut peer = ws_client::confirm_pairing(session, code)
        .await
        .map_err(|e| e.to_string())?;
    peer.host = host;
    peer.port = port;

    let dto = PairedWorkerDto {
        worker_id: peer.worker_id.clone(),
        worker_name: peer.worker_name.clone(),
    };

    state
        .peer_store
        .lock()
        .await
        .add_and_save(peer)
        .map_err(|e| e.to_string())?;

    Ok(dto)
}

/// Opens a pinned connection to an already-paired worker and submits a
/// job, returning immediately with the generated `job_id`. The frontend
/// listens for `job://log`, `job://progress`, `job://artifact`, and
/// `job://finished` events rather than blocking this call on the whole
/// build.
#[tauri::command]
pub async fn submit_job(
    app: AppHandle,
    state: State<'_, AppState>,
    worker_id: String,
    repo: String,
    branch: String,
    profile: String,
) -> Result<String, String> {
    let peer: MasterPeer = state
        .peer_store
        .lock()
        .await
        .get(&worker_id)
        .cloned()
        .ok_or_else(|| "worker is not paired".to_string())?;

    let stream = ws_client::resume_paired_connection(&peer, &state.master_name, &state.master_id)
        .await
        .map_err(|e| e.to_string())?;

    let job_id = format!("job-{:016x}", rand::random::<u64>());
    let cancel = CancellationToken::new();
    state
        .active_jobs
        .lock()
        .await
        .insert(job_id.clone(), cancel.clone());

    let job_args = ws_client::JobRequestArgs {
        job_id: job_id.clone(),
        repo,
        branch,
        profile,
    };
    let artifacts_dir = state.runtime_dir.join("artifacts");
    let active_jobs = state.active_jobs.clone();
    let finished_job_id = job_id.clone();

    tokio::spawn(async move {
        let _ = ws_client::run_job_stream(stream, job_args, app, artifacts_dir, cancel).await;
        active_jobs.lock().await.remove(&finished_job_id);
    });

    Ok(job_id)
}

/// Signals cancellation for an in-flight job. A no-op (not an error) if
/// the job already finished — cancellation racing completion is expected,
/// not exceptional.
#[tauri::command]
pub async fn cancel_job(state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    if let Some(token) = state.active_jobs.lock().await.get(&job_id) {
        token.cancel();
    }
    Ok(())
}

/// Opens the folder a finished job's artifact was written to.
#[tauri::command]
pub async fn open_artifacts_folder(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<(), String> {
    let path = state.runtime_dir.join("artifacts").join(&job_id);
    tauri_plugin_opener::open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}
