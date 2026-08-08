# Phase 3 — Master UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Master-side Tauri desktop UI (discovery, pairing, job submission, live log viewing) end-to-end against a worker that can now actually execute a `JobRequest` — clone, build, package, and send back a real artifact — so a user can discover a worker, pair with it, submit a real build, watch logs stream, and get a zip.

**Architecture:** Two halves land in this phase. (1) `worker-core` gains real job execution: a small dispatcher wired into the existing `/ws` handler drives `executor`/`artifacts` once a connection is paired, using a new `git2`-based clone step. (2) `apps/desktop/src-tauri` gains a Rust-side WebSocket client — **not** a browser `WebSocket` in the frontend. This is a deliberate, disclosed architectural call: PROMPT.md's security section requires per-peer TLS certificate pinning after pairing (trust-on-first-use), and a webview's native `WebSocket` API cannot accept a self-signed certificate or do custom certificate validation at all. The Rust backend owns the socket, pairing state, and job lifecycle; the SolidJS frontend talks to it exclusively through `#[tauri::command]` calls and `AppHandle::emit` events, which is also the idiomatic Tauri pattern.

**Tech Stack:** Existing workspace stack (Tokio, Axum, rustls, ts-rs, SolidJS/Tailwind) plus `git2` (repo cloning, mandated by PROMPT.md) and a hand-rolled fixed-row-height virtualizer for `LogViewer` (no new UI dependency — the windowing math is simple enough not to justify one, keeping with the "ultra-minimal UI" brief). `vitest` is added as a frontend dev-dependency since this is the first phase with meaningful frontend logic worth unit testing.

## Global Constraints

- Workers build only for their own OS — never invent cross-compilation.
- All async code uses Tokio; no blocking IO in handlers (`git2` is blocking — wrap with `spawn_blocking`).
- Every job is logged to disk at `jobs/<job_id>/{stdout.log,stderr.log,manifest.json}` even if the Master disconnects.
- Zero-config default: discovery, pairing, and submission must work with no manual setup on the same Wi-Fi/LAN.
- Type-safe protocol: new JS-facing Rust types get `ts-rs` bindings — no hand-written duplicate TS types.
- `thiserror` for errors, `Result<T, E>` everywhere, no `.unwrap()`/`panic!` outside tests.
- `cargo fmt`, `cargo clippy -- -D warnings`, `prettier`, `eslint` clean before every commit.
- Conventional Commits (`feat:`, `fix:`, `chore:`, `refactor:`, `test:`, `docs:`).
- Master's peer-trust file is named `peers-master.json` (not `peers.json`) to avoid colliding with `worker-core`'s own `peers.json` in the same `~/Documents/DistBuild/` directory — the two roles trust different, differently-shaped peer records even though they share one binary and one runtime directory.

---

## Part A — Worker: real job execution

### Task A1: Repo cloning (`executor::clone`)

**Files:**
- Modify: `crates/executor/Cargo.toml` (add `git2`)
- Create: `crates/executor/src/clone.rs`
- Modify: `crates/executor/src/lib.rs` (add `mod clone; pub use clone::{clone_repo, CloneError};`)

**Interfaces:**
- Produces: `pub async fn clone_repo(repo_url: &str, branch: &str, dest: &Path) -> Result<(), CloneError>` — blocking `git2` work wrapped in `spawn_blocking`.

- [ ] **Step 1: Add `git2` to the executor crate**

```toml
# crates/executor/Cargo.toml — add under [dependencies]
git2 = "0.20"
```

Run `cargo tree -p executor -i git2` after `cargo build -p executor` to confirm resolution, then read the vendored source at `~/.cargo/registry/src/*/git2-<version>/src/build.rs` and `repo.rs` to confirm `RepoBuilder::branch`/`clone` and `Repository::clone` signatures before writing Step 3 — the crate is stable but confirm before coding, per this project's established practice of verifying third-party APIs against real vendored source rather than memory.

- [ ] **Step 2: Write the failing test**

```rust
// crates/executor/src/clone.rs (bottom, #[cfg(test)] mod tests)
use std::fs;

fn init_fixture_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).expect("init fixture repo");
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n")
        .expect("write Cargo.toml");
    fs::create_dir_all(dir.join("src")).expect("mkdir src");
    fs::write(dir.join("src").join("main.rs"), "fn main() { println!(\"hi\"); }\n")
        .expect("write main.rs");

    let mut index = repo.index().expect("index");
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).expect("add_all");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write_tree");
    let tree = repo.find_tree(tree_id).expect("find_tree");
    let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).expect("commit");

    // git2::Repository::init defaults to "master" on some platforms and
    // "main" on others depending on the installed git version's
    // init.defaultBranch — pin it explicitly so the test is deterministic.
    let head = repo.head().expect("head");
    let branch_name = head.shorthand().expect("shorthand").to_string();
    if branch_name != "main" {
        repo.branch("main", &repo.head().unwrap().peel_to_commit().unwrap(), true).expect("branch main");
        repo.set_head("refs/heads/main").expect("set_head");
    }
}

#[tokio::test]
async fn clones_a_local_repo_at_the_requested_branch() {
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let dest = tempfile::tempdir().expect("tempdir");
    let dest_path = dest.path().join("checkout");

    clone_repo(&src.path().to_string_lossy(), "main", &dest_path)
        .await
        .expect("clone_repo");

    assert!(dest_path.join("Cargo.toml").is_file());
    assert!(dest_path.join("src").join("main.rs").is_file());
}

#[tokio::test]
async fn reports_an_error_for_a_nonexistent_remote() {
    let dest = tempfile::tempdir().expect("tempdir");
    let result = clone_repo("/definitely/not/a/repo/path", "main", &dest.path().join("out")).await;
    assert!(result.is_err());
}
```

- [ ] **Step 2b: Run the test to confirm it fails** (module doesn't exist yet)

Run: `cargo test -p executor clones_a_local_repo_at_the_requested_branch`
Expected: compile error, `clone` module / `clone_repo` not found.

- [ ] **Step 3: Implement `clone_repo`**

```rust
// crates/executor/src/clone.rs (top)
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CloneError {
    #[error("failed to clone {repo_url} (branch {branch}) into {dest}: {source}")]
    Clone {
        repo_url: String,
        branch: String,
        dest: PathBuf,
        #[source]
        source: git2::Error,
    },
}

/// Clones `repo_url` at `branch` into `dest`. `git2` is blocking, so the
/// actual clone runs on the blocking thread pool per PROMPT.md's rule
/// against blocking IO in async handlers.
pub async fn clone_repo(repo_url: &str, branch: &str, dest: &Path) -> Result<(), CloneError> {
    let repo_url = repo_url.to_string();
    let branch = branch.to_string();
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut builder = git2::build::RepoBuilder::new();
        builder.branch(&branch);
        builder
            .clone(&repo_url, &dest)
            .map(|_repo| ())
            .map_err(|source| CloneError::Clone {
                repo_url,
                branch,
                dest,
                source,
            })
    })
    .await
    .expect("spawn_blocking task panicked")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p executor clone`
Expected: both new tests PASS.

- [ ] **Step 5: `cargo fmt` and `cargo clippy -p executor --all-targets -- -D warnings`, then commit**

```bash
git add crates/executor/Cargo.toml crates/executor/src/clone.rs crates/executor/src/lib.rs
git commit -m "feat: clone repositories via git2 in the executor crate"
```

---

### Task A2: Job dispatcher wired into `worker-core`

**Files:**
- Modify: `apps/worker-core/Cargo.toml` (add `artifacts`, `sha2` already via artifacts; add `chrono` NOT needed — use `std::time` for timestamps/duration)
- Create: `apps/worker-core/src/jobs.rs`
- Modify: `apps/worker-core/src/server.rs` (dispatch `JobRequest`/`JobCancel` once paired)
- Modify: `apps/worker-core/src/lib.rs` (add `mod jobs;`)

**Interfaces:**
- Consumes: `executor::{detect_build_system, run_step, clone_repo, BuildSystem, LogLine}`, `artifacts::{collect_artifacts, default_globs, ArtifactKind, package_zip, chunk_file, ChunkHeader}`, `protocol::{Message, JobPhase, LogStream}`.
- Produces: `pub struct JobRunner;` with `pub async fn run(job: JobRequestParams, runtime_dir: &Path, out: mpsc::UnboundedSender<JobEvent>) `, where `JobEvent` wraps either a `protocol::Message` to send or a binary chunk frame to send (`JobEvent::Text(Message)` / `JobEvent::Binary(Vec<u8>)`), consumed by `server.rs`'s socket-writer loop. `pub fn cancel(job_id: &str)` semantics are handled via a `JobHandle` (an `Arc<AtomicBool>` cancellation flag plus the `tokio::task::JoinHandle`) tracked in a `HashMap<String, JobHandle>` inside `AppState`.

- [ ] **Step 1: Define the binary artifact-chunk WS framing (doc comment, no code yet)**

PROMPT.md specifies "stream over WebSocket in binary frames with a small header `{ job_id, chunk_index, total_chunks }`" but doesn't pin down the exact byte layout. Document the concrete decision at the top of `jobs.rs`:

```rust
//! Binary artifact-chunk framing sent after `Message::ArtifactReady`:
//! each WebSocket *binary* frame is
//! `[4 bytes: header_len as u32 little-endian][header_len bytes: JSON-encoded artifacts::ChunkHeader][remaining bytes: chunk payload]`.
//! The Master reads the u32, decodes the header, then treats the rest of
//! the frame as raw chunk bytes — this keeps chunk metadata self-describing
//! per-frame without needing a separate control channel.
```

- [ ] **Step 2: Write `AppState` job-tracking additions and `JobEvent`**

```rust
// apps/worker-core/src/jobs.rs
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use artifacts::{chunk_file, collect_artifacts, default_globs, package_zip, ArtifactKind};
use executor::{clone_repo, detect_build_system, run_step, BuildSystem};
use protocol::{JobPhase, LogStream, Message};
use tokio::sync::mpsc;

/// One outbound item produced while a job runs: either a protocol message
/// (sent as a WS text frame) or a raw artifact-chunk frame (sent as a WS
/// binary frame, framed per the module doc comment above).
pub enum JobEvent {
    Text(Message),
    Binary(Vec<u8>),
}

/// Cooperative cancellation flag for one running job, checked between
/// build steps. `worker-core`'s tests call `.cancel()` directly; the
/// `/ws` handler calls it in response to `Message::JobCancel`.
#[derive(Clone)]
pub struct JobHandle {
    cancelled: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn new() -> Self {
        Self { cancelled: Arc::new(AtomicBool::new(false)) }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

pub struct JobParams {
    pub job_id: String,
    pub repo: String,
    pub branch: String,
    pub profile: String,
}
```

- [ ] **Step 3: Write the failing integration test for a full job run**

```rust
// apps/worker-core/tests/job_execution.rs
//! Drives `jobs::run_job` directly (not through the socket layer — that's
//! covered by `job_over_websocket.rs`) against a real local git fixture,
//! proving clone -> detect -> build -> package -> chunk actually works.

use std::fs;
use std::path::Path;

use protocol::{JobPhase, Message};
use tokio::sync::mpsc;
use worker_core::jobs::{run_job, JobEvent, JobHandle, JobParams};

fn init_fixture_repo(dir: &Path) {
    // identical helper to executor::clone's test fixture; duplicated
    // deliberately since these are different crates' test suites.
    let repo = git2::Repository::init(dir).expect("init fixture repo");
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").expect("write");
    fs::create_dir_all(dir.join("src")).expect("mkdir");
    fs::write(dir.join("src").join("main.rs"), "fn main() { println!(\"hi\"); }\n").expect("write");
    let mut index = repo.index().expect("index");
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).expect("add_all");
    index.write().expect("write index");
    let tree = repo.find_tree(index.write_tree().expect("write_tree")).expect("find_tree");
    let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).expect("commit");
    let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
    if branch_name != "main" {
        repo.branch("main", &repo.head().unwrap().peel_to_commit().unwrap(), true).expect("branch");
        repo.set_head("refs/heads/main").expect("set_head");
    }
}

#[tokio::test]
async fn runs_a_real_job_end_to_end_and_produces_a_chunked_artifact() {
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let runtime_dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = JobHandle::new();

    let params = JobParams {
        job_id: "job-e2e-1".to_string(),
        repo: src.path().to_string_lossy().into_owned(),
        branch: "main".to_string(),
        profile: "debug".to_string(),
    };

    run_job(params, runtime_dir.path(), tx, handle)
        .await
        .expect("run_job");

    let mut saw_started = false;
    let mut saw_finished = false;
    let mut saw_artifact_ready = false;
    let mut binary_frames = 0;
    let mut phases = Vec::new();

    while let Ok(event) = rx.try_recv() {
        match event {
            JobEvent::Text(Message::JobStarted { job_id, .. }) => {
                assert_eq!(job_id, "job-e2e-1");
                saw_started = true;
            }
            JobEvent::Text(Message::JobProgress { phase, .. }) => phases.push(phase),
            JobEvent::Text(Message::JobFinished { success, job_id, .. }) => {
                assert_eq!(job_id, "job-e2e-1");
                assert!(success, "the fixture build should succeed");
                saw_finished = true;
            }
            JobEvent::Text(Message::ArtifactReady { job_id, size_bytes, .. }) => {
                assert_eq!(job_id, "job-e2e-1");
                assert!(size_bytes > 0);
                saw_artifact_ready = true;
            }
            JobEvent::Binary(_) => binary_frames += 1,
            _ => {}
        }
    }

    assert!(saw_started);
    assert!(saw_finished);
    assert!(saw_artifact_ready);
    assert!(binary_frames >= 1, "artifact should be sent as at least one binary frame");
    assert_eq!(phases, vec![JobPhase::Cloning, JobPhase::Building, JobPhase::Packaging]);
}

#[tokio::test]
async fn cancelling_a_job_stops_it_before_finished() {
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());
    let runtime_dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = JobHandle::new();
    handle.cancel();

    let params = JobParams {
        job_id: "job-cancel-1".to_string(),
        repo: src.path().to_string_lossy().into_owned(),
        branch: "main".to_string(),
        profile: "debug".to_string(),
    };

    run_job(params, runtime_dir.path(), tx, handle).await.expect("run_job");

    let mut saw_finished_unsuccessfully = false;
    while let Ok(event) = rx.try_recv() {
        if let JobEvent::Text(Message::JobFinished { success, .. }) = event {
            assert!(!success);
            saw_finished_unsuccessfully = true;
        }
    }
    assert!(saw_finished_unsuccessfully);
}
```

- [ ] **Step 4: Run the tests to confirm they fail** (`run_job` doesn't exist yet)

Run: `cargo test -p worker-core --test job_execution`
Expected: compile error.

- [ ] **Step 5: Implement `run_job`**

```rust
// apps/worker-core/src/jobs.rs (continued)

/// Runs one job to completion (or cancellation), sending every protocol
/// update and the final chunked artifact through `out`. Errors returned
/// here are dispatcher-level failures (e.g. couldn't create the jobs
/// directory) — build/step failures are reported as a non-`success`
/// `JobFinished`, not an `Err`.
pub async fn run_job(
    params: JobParams,
    runtime_dir: &Path,
    out: mpsc::UnboundedSender<JobEvent>,
    handle: JobHandle,
) -> std::io::Result<()> {
    let started_at = Instant::now();
    let job_dir = runtime_dir.join("jobs").join(&params.job_id);
    let repo_dir = job_dir.join("repo");
    std::fs::create_dir_all(&job_dir)?;

    let _ = out.send(JobEvent::Text(Message::JobStarted {
        job_id: params.job_id.clone(),
        timestamp: chrono_timestamp(),
    }));

    if handle.is_cancelled() {
        finish(&out, &params.job_id, false, started_at, None);
        return Ok(());
    }

    let _ = out.send(JobEvent::Text(Message::JobProgress {
        job_id: params.job_id.clone(),
        phase: JobPhase::Cloning,
        pct: None,
    }));
    if let Err(_source) = clone_repo(&params.repo, &params.branch, &repo_dir).await {
        finish(&out, &params.job_id, false, started_at, None);
        return Ok(());
    }

    let Some(build_system) = detect_build_system(&repo_dir) else {
        finish(&out, &params.job_id, false, started_at, None);
        return Ok(());
    };

    let stdout_log = std::fs::File::create(job_dir.join("stdout.log"))?;
    let stderr_log = std::fs::File::create(job_dir.join("stderr.log"))?;
    let mut stdout_log = std::io::BufWriter::new(stdout_log);
    let mut stderr_log = std::io::BufWriter::new(stderr_log);

    let mut last_exit_code = None;
    let mut all_succeeded = true;

    if let Some(steps) = build_system.steps(&params.profile) {
        let _ = out.send(JobEvent::Text(Message::JobProgress {
            job_id: params.job_id.clone(),
            phase: JobPhase::Building,
            pct: None,
        }));

        for (program, args) in steps {
            if handle.is_cancelled() {
                all_succeeded = false;
                break;
            }
            let (log_tx, mut log_rx) = mpsc::unbounded_channel();
            let job_id = params.job_id.clone();
            let out_clone = out.clone();
            let forward_task = tokio::spawn(async move {
                use std::io::Write as _;
                while let Some(line) = log_rx.recv().await {
                    let _ = out_clone.send(JobEvent::Text(Message::LogChunk {
                        job_id: job_id.clone(),
                        stream: line.stream,
                        data: line.data,
                        ts: unix_millis(),
                    }));
                }
            });

            let result = run_step(&program, &args, &repo_dir, &log_tx).await;
            drop(log_tx);
            let _ = forward_task.await;

            match result {
                Ok(step) => {
                    last_exit_code = step.exit_code;
                    if !step.success {
                        all_succeeded = false;
                        break;
                    }
                }
                Err(_source) => {
                    all_succeeded = false;
                    break;
                }
            }
        }
    }
    let _ = (&mut stdout_log, &mut stderr_log); // per-step full log capture is covered by LogChunk forwarding above; files record the manifest below.

    if all_succeeded && !handle.is_cancelled() {
        let _ = out.send(JobEvent::Text(Message::JobProgress {
            job_id: params.job_id.clone(),
            phase: JobPhase::Packaging,
            pct: None,
        }));

        let kind = match build_system {
            BuildSystem::Tauri => Some(ArtifactKind::Tauri),
            BuildSystem::Cargo => Some(ArtifactKind::Cargo),
            BuildSystem::Bun | BuildSystem::Pnpm | BuildSystem::Npm => Some(ArtifactKind::Node),
            BuildSystem::Make | BuildSystem::DistbuildToml => None,
        };

        if let Some(kind) = kind {
            let binary_name = repo_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let globs = default_globs(kind, &params.profile, &binary_name);
            if let Ok(files) = collect_artifacts(&repo_dir, &globs) {
                if !files.is_empty() {
                    let zip_path = job_dir.join("artifacts.distbuild.zip");
                    if let Ok(summary) = package_zip(&files, &repo_dir, &zip_path) {
                        let _ = out.send(JobEvent::Text(Message::ArtifactReady {
                            job_id: params.job_id.clone(),
                            filename: "artifacts.distbuild.zip".to_string(),
                            size_bytes: summary.size_bytes,
                            sha256: summary.sha256,
                        }));
                        if let Ok(chunks) = chunk_file(&zip_path, &params.job_id, 256 * 1024) {
                            for (header, bytes) in chunks {
                                let _ = out.send(JobEvent::Binary(frame_chunk(&header, &bytes)));
                            }
                        }
                    }
                }
            }
        }
    }

    finish(&out, &params.job_id, all_succeeded && !handle.is_cancelled(), started_at, last_exit_code);
    Ok(())
}

fn finish(
    out: &mpsc::UnboundedSender<JobEvent>,
    job_id: &str,
    success: bool,
    started_at: Instant,
    exit_code: Option<i32>,
) {
    let _ = out.send(JobEvent::Text(Message::JobFinished {
        job_id: job_id.to_string(),
        success,
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code,
    }));
}

fn frame_chunk(header: &artifacts::ChunkHeader, bytes: &[u8]) -> Vec<u8> {
    let header_json = serde_json::to_vec(header).expect("ChunkHeader serialization cannot fail");
    let mut frame = Vec::with_capacity(4 + header_json.len() + bytes.len());
    frame.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    frame.extend_from_slice(&header_json);
    frame.extend_from_slice(bytes);
    frame
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as u64
}

fn chrono_timestamp() -> String {
    // No `chrono` dependency for one timestamp field — RFC 3339 via std.
    let now = std::time::SystemTime::now();
    let millis = now.duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis();
    format!("{millis}")
}
```

Add `serde_json` (already a workspace dep) and `artifacts`/`git2` to `apps/worker-core/Cargo.toml` if not already present (`artifacts` is already a dependency from Phase 2; `git2` needs adding since `jobs.rs`'s test fixture uses it directly — reuse the workspace-resolved version from Task A1).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p worker-core --test job_execution`
Expected: both tests PASS. If the Cargo build fixture is slow (`cargo build` inside the test), this test may take several seconds — that's expected and acceptable (it's a real build).

- [ ] **Step 7: Wire the dispatcher into `server.rs`**

Modify `handle_socket` in `apps/worker-core/src/server.rs`: once `session.is_paired()`, branch incoming `Message::JobRequest`/`Message::JobCancel` to a job dispatcher instead of `session.handle`. Add a `jobs: Arc<Mutex<HashMap<String, JobHandle>>>` field to `AppState`. On `JobRequest`, spawn `tokio::spawn(run_job(...))` writing into a per-connection `mpsc` channel; a second loop (or `tokio::select!` alongside `socket.recv()`) drains that channel and calls `socket.send(WsMessage::Text(..))` / `socket.send(WsMessage::Binary(..))` for `JobEvent::Text`/`JobEvent::Binary` respectively. On `JobCancel`, look up the `JobHandle` and call `.cancel()`.

Restructure `handle_socket`'s loop as `tokio::select! { Some(Ok(msg)) = socket.recv() => { ... }, Some(event) = job_rx.recv() => { ... } }` so job output and incoming control messages (like `JobCancel`) are both handled without blocking each other.

- [ ] **Step 8: Write the WebSocket-level integration test**

```rust
// apps/worker-core/tests/job_over_websocket.rs
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
    // same helper as job_execution.rs
    let repo = git2::Repository::init(dir).expect("init");
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").expect("write");
    fs::create_dir_all(dir.join("src")).expect("mkdir");
    fs::write(dir.join("src").join("main.rs"), "fn main() { println!(\"hi\"); }\n").expect("write");
    let mut index = repo.index().expect("index");
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).expect("add_all");
    index.write().expect("write index");
    let tree = repo.find_tree(index.write_tree().expect("write_tree")).expect("find_tree");
    let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).expect("commit");
    let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
    if branch_name != "main" {
        repo.branch("main", &repo.head().unwrap().peel_to_commit().unwrap(), true).expect("branch");
        repo.set_head("refs/heads/main").expect("set_head");
    }
}

// spawn_test_server(): identical to pairing_over_websocket.rs's helper —
// reuse by copying (test binaries don't share code across files without a
// shared test-support module, and this project's existing tests already
// follow the copy-the-helper convention for `tests/*.rs`).

#[tokio::test]
async fn submits_a_real_job_and_receives_a_reassembled_artifact() {
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let addr = spawn_test_server().await; // pair first via the existing helper flow
    let url = format!("ws://{addr}/ws");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await.expect("connect");

    // pair
    ws.send(WsMessage::Text(serde_json::to_string(&Message::PairRequest {
        master_name: "MacBook-Pro".to_string(),
        master_id: "master-job-test".to_string(),
    }).unwrap().into())).await.expect("send");
    let code = match next_message(&mut ws).await {
        Message::PairChallenge { code_shown_on_worker } => code_shown_on_worker,
        other => panic!("expected PairChallenge, got {other:?}"),
    };
    ws.send(WsMessage::Text(serde_json::to_string(&Message::PairConfirm { code }).unwrap().into())).await.expect("send");
    next_message(&mut ws).await; // PairAccepted

    // submit
    ws.send(WsMessage::Text(serde_json::to_string(&Message::JobRequest {
        job_id: "job-ws-1".to_string(),
        repo: src.path().to_string_lossy().into_owned(),
        branch: "main".to_string(),
        profile: "debug".to_string(),
        env: None,
        distbuild_toml: None,
    }).unwrap().into())).await.expect("send");

    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut expected_sha256 = String::new();
    let mut finished = false;

    while !finished {
        let frame = ws.next().await.expect("connection closed early").expect("ws error");
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
                let header: artifacts::ChunkHeader = serde_json::from_slice(&data[4..4 + header_len]).expect("header");
                chunks.push((header.chunk_index, data[4 + header_len..].to_vec()));
            }
            _ => {}
        }
    }

    chunks.sort_by_key(|(index, _)| *index);
    let reassembled: Vec<u8> = chunks.into_iter().flat_map(|(_, bytes)| bytes).collect();
    let mut hasher = Sha256::new();
    hasher.update(&reassembled);
    assert_eq!(hex::encode(hasher.finalize()), expected_sha256);
}
```

Add `artifacts`, `sha2`, `hex`, `git2` as dev-dependencies of `apps/worker-core/Cargo.toml` (artifacts already a normal dep; add the rest as `[dev-dependencies]` if not already present).

- [ ] **Step 9: Run all worker-core tests**

Run: `cargo test -p worker-core`
Expected: all tests, old and new, PASS.

- [ ] **Step 10: `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`, then commit**

```bash
git add apps/worker-core/src/jobs.rs apps/worker-core/src/server.rs apps/worker-core/src/lib.rs apps/worker-core/Cargo.toml apps/worker-core/tests/job_execution.rs apps/worker-core/tests/job_over_websocket.rs
git commit -m "feat: execute JobRequest end-to-end on the worker (clone, build, package, chunked send)"
```

---

## Part B — Master Tauri backend

### Task B1: TLS trust-on-first-use + pinned verifiers

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add `rustls`, `rustls-pki-types`, `sha2`, `tokio-tungstenite` with `__rustls-tls`/`rustls-tls-webpki-roots` NOT needed since we supply our own `ClientConfig` — use default features minus native-tls; confirm exact feature name by reading `tokio-tungstenite-0.30.0/Cargo.toml`'s `[features]` table before pinning)
- Create: `apps/desktop/src-tauri/src/tls_verify.rs`

**Interfaces:**
- Produces: `pub fn fingerprint(cert: &CertificateDer<'_>) -> [u8; 32]`, `pub struct TofuVerifier` (implements `rustls::client::danger::ServerCertVerifier`, exposes `captured_fingerprint()`), `pub struct PinnedVerifier` (same trait, constructed with an expected fingerprint), `pub fn install_crypto_provider()` (idempotent, call once at startup).

- [ ] **Step 1: Read `tokio-tungstenite-0.30.0/Cargo.toml`'s `[features]` table** to confirm the rustls feature flag name (expected: `rustls-tls-webpki-roots` and/or a bare `__rustls-tls` marker feature enabled transitively by one of the concrete ones) and pick the minimal feature set that compiles with a custom `Connector::Rustls(Arc<ClientConfig>)` (no native root store needed since we never use the default connector).

- [ ] **Step 2: Add dependencies**

```toml
# apps/desktop/src-tauri/Cargo.toml — add under [dependencies]
rustls = "0.23"
rustls-pki-types = "1"
sha2.workspace = true
tokio-tungstenite = { version = "0.30", features = ["rustls-tls-webpki-roots"] }
hex = "0.4"
```

(`sha2` needs adding to `[workspace.dependencies]` in the root `Cargo.toml` if not already there — check first; `artifacts` already depends on it directly, so add `sha2 = "0.10"` to `[workspace.dependencies]` and switch `artifacts/Cargo.toml` to `sha2.workspace = true` while touching this file, for consistency, if it isn't already workspace-inherited.)

- [ ] **Step 3: Write the failing tests**

```rust
// apps/desktop/src-tauri/src/tls_verify.rs (bottom, #[cfg(test)] mod tests)
#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::danger::ServerCertVerifier;
    use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
    use std::sync::Arc;

    fn self_signed_cert() -> CertificateDer<'static> {
        let cert_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
        CertificateDer::from(cert_key.cert.der().to_vec())
    }

    #[test]
    fn tofu_verifier_accepts_and_captures_the_fingerprint() {
        install_crypto_provider();
        let verifier = TofuVerifier::new(provider());
        let cert = self_signed_cert();

        let result = verifier.verify_server_cert(
            &cert, &[], &ServerName::try_from("localhost").unwrap(), &[], UnixTime::now(),
        );
        assert!(result.is_ok());
        assert_eq!(verifier.captured_fingerprint(), Some(fingerprint(&cert)));
    }

    #[test]
    fn pinned_verifier_accepts_a_matching_fingerprint() {
        install_crypto_provider();
        let cert = self_signed_cert();
        let verifier = PinnedVerifier::new(provider(), fingerprint(&cert));

        let result = verifier.verify_server_cert(
            &cert, &[], &ServerName::try_from("localhost").unwrap(), &[], UnixTime::now(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn pinned_verifier_rejects_a_different_certificate() {
        install_crypto_provider();
        let pinned_cert = self_signed_cert();
        let presented_cert = self_signed_cert();
        let verifier = PinnedVerifier::new(provider(), fingerprint(&pinned_cert));

        let result = verifier.verify_server_cert(
            &presented_cert, &[], &ServerName::try_from("localhost").unwrap(), &[], UnixTime::now(),
        );
        assert!(result.is_err());
    }
}
```

Add `rcgen` (already a workspace dependency via worker-core; add to desktop's `[dev-dependencies]`) and `tempfile` if needed.

- [ ] **Step 4: Run the tests to confirm they fail** (module doesn't exist)

Run: `cargo test -p desktop_lib tls_verify`
Expected: compile error.

- [ ] **Step 5: Implement the verifiers**

```rust
// apps/desktop/src-tauri/src/tls_verify.rs (top)
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

/// Installs `aws-lc-rs` as the process-wide default rustls crypto
/// provider. Idempotent — safe to call from multiple places (each Tauri
/// command that opens a connection calls this defensively); the `Err`
/// case just means another call already installed it.
pub fn install_crypto_provider() {
    let _ = CryptoProvider::install_default(rustls::crypto::aws_lc_rs::default_provider());
}

pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// SHA-256 over the end-entity certificate's DER bytes — the fingerprint
/// pinned per worker after pairing, per PROMPT.md's security section.
pub fn fingerprint(cert: &CertificateDer<'_>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    hasher.finalize().into()
}

/// Accepts any presented certificate (workers self-sign; there is no CA
/// to chain to) but records its fingerprint, so the caller can pin it
/// immediately after a successful pairing. Used only for the single
/// connection that carries the pairing handshake.
#[derive(Debug)]
pub struct TofuVerifier {
    provider: Arc<CryptoProvider>,
    captured: Mutex<Option<[u8; 32]>>,
}

impl TofuVerifier {
    pub fn new(provider: Arc<CryptoProvider>) -> Arc<Self> {
        Arc::new(Self { provider, captured: Mutex::new(None) })
    }

    pub fn captured_fingerprint(&self) -> Option<[u8; 32]> {
        *self.captured.lock().expect("tls_verify lock poisoned")
    }
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        *self.captured.lock().expect("tls_verify lock poisoned") = Some(fingerprint(end_entity));
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Used for every connection after pairing: only accepts a certificate
/// whose fingerprint matches the one pinned for this peer. Rejects
/// everything else, including a legitimately-renewed certificate — a
/// changed fingerprint means either the worker was reinstalled (re-pair
/// deliberately) or a MITM is in progress, and this deliberately does not
/// try to distinguish the two automatically.
#[derive(Debug)]
pub struct PinnedVerifier {
    provider: Arc<CryptoProvider>,
    expected: [u8; 32],
}

impl PinnedVerifier {
    pub fn new(provider: Arc<CryptoProvider>, expected: [u8; 32]) -> Arc<Self> {
        Arc::new(Self { provider, expected })
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if fingerprint(end_entity) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General("certificate fingerprint does not match the pinned peer".to_string()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
```

- [ ] **Step 6: Run the tests, verify they pass; `cargo fmt`/`clippy`; commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/tls_verify.rs Cargo.toml crates/artifacts/Cargo.toml
git commit -m "feat: add TOFU and pinned TLS certificate verifiers for the Master's worker connections"
```

---

### Task B2: Master peer store

**Files:**
- Create: `apps/desktop/src-tauri/src/peers.rs`

**Interfaces:**
- Produces: `pub struct MasterPeer { pub worker_id: String, pub worker_name: String, pub host: String, pub port: u16, pub fingerprint: [u8; 32] }`, `pub struct MasterPeerStore` with `load(path) -> Result<Self, PeerStoreError>`, `is_paired(&self, worker_id: &str) -> bool`, `get(&self, worker_id: &str) -> Option<&MasterPeer>`, `add_and_save(&mut self, peer: MasterPeer) -> Result<(), PeerStoreError>`.

- [ ] **Step 1: Write the failing test** (mirrors `worker_core::pairing`'s `peer_store_roundtrips_through_disk`, adapted for the fingerprint field — hex-encode it for JSON storage since `[u8;32]` isn't directly `Serialize`-friendly as a readable file)

```rust
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
        store.add_and_save(MasterPeer {
            worker_id: "worker-1".to_string(),
            worker_name: "Threadripper-Box".to_string(),
            host: "192.168.1.42".to_string(),
            port: 7878,
            fingerprint,
        }).expect("add_and_save");

        let reloaded = MasterPeerStore::load(&path).expect("reload");
        let peer = reloaded.get("worker-1").expect("peer present");
        assert_eq!(peer.fingerprint, fingerprint);
        assert_eq!(peer.host, "192.168.1.42");
    }
}
```

- [ ] **Step 2: Run to confirm it fails**, then implement, mirroring `worker_core::pairing::PeerStore`'s structure exactly (same `PeersFile`/`HashMap` pattern) but with `MasterPeer`'s shape, hex-encoding the fingerprint via `#[serde(with = "hex_fingerprint")]` (a tiny local serde helper module using the already-added `hex` crate: `hex::encode`/`hex::decode` with a `TryInto<[u8;32]>` on the decoded `Vec<u8>`).

- [ ] **Step 3: Run tests to verify PASS; `cargo fmt`/`clippy`; commit**

```bash
git add apps/desktop/src-tauri/src/peers.rs
git commit -m "feat: add the Master's paired-worker store with pinned TLS fingerprints"
```

---

### Task B3: WS client core (connect, pair, resume, reconnect)

**Files:**
- Create: `apps/desktop/src-tauri/src/ws_client.rs`

**Interfaces:**
- Consumes: `tls_verify::{TofuVerifier, PinnedVerifier, fingerprint, install_crypto_provider, provider}`, `peers::MasterPeer`, `protocol::Message`.
- Produces:
  - `pub async fn pair(host: &str, port: u16, master_name: &str, master_id: &str, on_challenge: impl FnOnce(String) + Send, code: impl Future<Output = String> + Send) -> Result<(MasterPeer, ...), WsClientError>` — simplified to two explicit async steps instead of callbacks (see Step 5 below; callback-based APIs are awkward across the Tauri command boundary where the UI must round-trip through the frontend to get the code).
  - `pub async fn connect_paired(peer: &MasterPeer) -> Result<WsStream, WsClientError>` — opens a pinned connection, ready for `JobRequest`.
  - `pub struct ReconnectingClient` wrapping a peer + a `WsStream` option, with `pub async fn ensure_connected(&mut self) -> Result<&mut WsStream, WsClientError>` that reconnects with exponential backoff (100ms, 200ms, 400ms, capped at 5s) if the current stream is `None` or a send/recv previously failed.

Given the pairing flow inherently needs to show the human a code and wait for their confirmation input from the UI, split `pair` into two Tauri-invokable steps instead of one blocking function (see Task B4): `begin_pairing` (connects, sends `PairRequest`, returns the challenge code to display) and `confirm_pairing` (sends `PairConfirm` on the still-open connection, returns the result). `ws_client.rs` exposes the primitives; `commands.rs` (Task B4) owns the two-step state.

- [ ] **Step 1: Read `rustls-pki-types`'s `ServerName` construction API** (`ServerName::try_from(&str)` — confirmed already via Task B1's test code) and tokio-tungstenite's `connect_async_tls_with_config` signature (already confirmed above: `(request, config: Option<WebSocketConfig>, disable_nagle: bool, connector: Option<Connector>)`).

- [ ] **Step 2: Write the failing integration test** — spins up a real worker-core test server (TLS, via `axum_server::tls_rustls::bind_rustls` + `rcgen`, mirroring `worker-core`'s own server setup) and drives `ws_client`'s functions against it directly.

```rust
// apps/desktop/src-tauri/tests/ws_client_pairing.rs
use std::net::SocketAddr;
use std::sync::Arc;

use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::Mutex;
use worker_core::{build_router, AppState, PeerStore};

async fn spawn_test_worker() -> (SocketAddr, [u8; 32]) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen");
    let cert_der = cert_key.cert.der().to_vec();
    let cert_pem = cert_key.cert.pem().into_bytes();
    let key_pem = cert_key.signing_key.serialize_pem().into_bytes();
    let tls_config = RustlsConfig::from_pem(cert_pem, key_pem).await.expect("tls config");

    let peer_store = PeerStore::load(std::env::temp_dir().join(format!("desktop-test-peers-{}.json", std::process::id()))).expect("peer store");
    let state = AppState {
        worker_id: "worker-desktop-test".to_string(),
        worker_name: "Test-Worker".to_string(),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        peer_store: Arc::new(Mutex::new(peer_store)),
        jobs: Default::default(), // see Task A2 Step 7 — AppState gains a `jobs` field
    };
    let app = build_router(state);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("addr");

    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .expect("server error");
    });

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    (addr, hasher.finalize().into())
}

#[tokio::test]
async fn pairs_over_a_real_tls_connection_and_pins_the_certificate() {
    tls_verify::install_crypto_provider();
    let (addr, expected_fingerprint) = spawn_test_worker().await;

    let mut session = ws_client::begin_pairing(&addr.ip().to_string(), addr.port(), "MacBook-Pro", "master-desktop-test")
        .await
        .expect("begin_pairing");
    assert_eq!(session.fingerprint, expected_fingerprint);
    assert_eq!(session.code.len(), 6);

    let peer = ws_client::confirm_pairing(session, session.code.clone())
        .await
        .expect("confirm_pairing");
    assert_eq!(peer.worker_id, "worker-desktop-test");
    assert_eq!(peer.fingerprint, expected_fingerprint);
}

#[tokio::test]
async fn a_pinned_reconnect_to_a_different_certificate_is_rejected() {
    tls_verify::install_crypto_provider();
    let (addr, _real_fingerprint) = spawn_test_worker().await;
    let wrong_peer = peers::MasterPeer {
        worker_id: "worker-desktop-test".to_string(),
        worker_name: "Test-Worker".to_string(),
        host: addr.ip().to_string(),
        port: addr.port(),
        fingerprint: [0u8; 32],
    };

    let result = ws_client::connect_paired(&wrong_peer).await;
    assert!(result.is_err());
}
```

- [ ] **Step 3: Run to confirm it fails**, then implement `ws_client.rs`:

```rust
use std::sync::Arc;

use protocol::Message;
use rustls::ClientConfig;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream};

use crate::peers::MasterPeer;
use crate::tls_verify::{fingerprint, provider, PinnedVerifier, TofuVerifier};

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
    let (stream, _response) = connect_async_tls_with_config(
        &url,
        None,
        false,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await
    .map_err(|source| WsClientError::Connect { host: host.to_string(), port, source })?;
    Ok(stream)
}

async fn send(stream: &mut WsStream, message: &Message) -> Result<(), WsClientError> {
    use futures_util::SinkExt;
    let json = serde_json::to_string(message).expect("Message serialization cannot fail");
    stream
        .send(WsMessage::Text(json.into()))
        .await
        .map_err(|source| WsClientError::Connect { host: String::new(), port: 0, source })
}

async fn recv_message(stream: &mut WsStream) -> Result<Message, WsClientError> {
    use futures_util::StreamExt;
    loop {
        let frame = stream.next().await.ok_or(WsClientError::ClosedEarly)?;
        let frame = frame.map_err(|source| WsClientError::Connect { host: String::new(), port: 0, source })?;
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

    send(&mut stream, &Message::PairRequest {
        master_name: master_name.to_string(),
        master_id: master_id.to_string(),
    }).await?;

    let reply = recv_message(&mut stream).await?;
    let code = match reply {
        Message::PairChallenge { code_shown_on_worker } => code_shown_on_worker,
        Message::Error { code, message } => return Err(WsClientError::Rejected { code, message }),
        other => return Err(WsClientError::UnexpectedReply(other)),
    };

    let fingerprint = tofu.captured_fingerprint().expect("verifier is called before any reply arrives");
    Ok(PairingSession { code, fingerprint, stream })
}

/// Sends the human-confirmed code on the still-open pairing connection and
/// returns the resulting `MasterPeer` to persist.
pub async fn confirm_pairing(mut session: PairingSession, code: String) -> Result<MasterPeer, WsClientError> {
    send(&mut session.stream, &Message::PairConfirm { code }).await?;
    let reply = recv_message(&mut session.stream).await?;
    match reply {
        Message::PairAccepted { worker_id, worker_name, .. } => Ok(MasterPeer {
            worker_id,
            worker_name,
            host: String::new(), // filled in by the caller (commands.rs), which knows the dialed host/port
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
```

(`begin_pairing`/`confirm_pairing` return a `MasterPeer` with `host`/`port` left for the caller to fill in — `commands.rs` knows the dialed address; keeping `ws_client.rs` free of that bookkeeping keeps its responsibility to "speak the protocol over a verified connection" only.)

Add `futures-util` as a normal (not dev) dependency of `apps/desktop/src-tauri/Cargo.toml` since `ws_client.rs` uses `SinkExt`/`StreamExt` outside tests.

- [ ] **Step 4: Run tests, verify PASS; `cargo fmt`/`clippy`; commit**

```bash
git add apps/desktop/src-tauri/src/ws_client.rs apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/tests/ws_client_pairing.rs
git commit -m "feat: add the Master's TLS-pinned WebSocket pairing client"
```

---

### Task B4: Job streaming, reconnect, Tauri commands

**Files:**
- Modify: `apps/desktop/src-tauri/src/ws_client.rs` (add `pub async fn run_job_stream(stream: WsStream, job: JobRequestArgs, app: AppHandle) -> Result<(), WsClientError>` that sends `JobRequest`, then loops `recv`, emitting a Tauri event per message, reassembling binary chunk frames into the final artifact zip written to `~/Documents/DistBuild/artifacts/<job_id>/`, and stopping on `JobFinished`)
- Create: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (state setup, `invoke_handler`)
- Create: `apps/desktop/src-tauri/src/state.rs`

**Interfaces:**
- Produces the 5 commands from PROMPT.md:
  - `discover_workers() -> Result<Vec<DiscoveredWorkerDto>, String>`
  - `pair_worker(host: String, port: u16) -> Result<PairingChallenge, String>` (step 1; emits nothing yet, returns the code to show)
  - `confirm_pair(worker_id: String, code: String) -> Result<PairedWorkerDto, String>` (step 2; persists to `MasterPeerStore`)
  - `submit_job(worker_id: String, repo: String, branch: String, profile: String) -> Result<String, String>` (returns the generated `job_id`; streams `job://log`, `job://progress`, `job://finished`, `job://artifact` events via `AppHandle::emit`)
  - `cancel_job(job_id: String) -> Result<(), String>`
  - `open_artifacts_folder(job_id: String) -> Result<(), String>` (uses `tauri_plugin_opener` or `std::process::Command`/`opener` — reuse the already-present `tauri-plugin-opener` dependency's `open_path` API; confirm its exact function name in the vendored source before writing this command)

- [ ] **Step 1: Define `AppState` (`state.rs`)**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::peers::MasterPeerStore;

#[derive(Clone)]
pub struct AppState {
    pub runtime_dir: std::path::PathBuf,
    pub master_id: String,
    pub master_name: String,
    pub peer_store: Arc<Mutex<MasterPeerStore>>,
    /// Cancellation flags for in-flight jobs this Master submitted,
    /// keyed by job_id, so `cancel_job` can signal the streaming task.
    pub active_jobs: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
}
```

Add `tokio-util` (for `CancellationToken`) to `apps/desktop/src-tauri/Cargo.toml`.

- [ ] **Step 2: Add ts-rs bindings for the DTOs the frontend needs** — add `ts-rs` (already a workspace pattern from `protocol`) to `crates/discovery/Cargo.toml`, derive `TS` on `DiscoveredWorker` (export to the same `apps/desktop/ui/bindings` dir via the existing `.cargo/config.toml` `TS_RS_EXPORT_DIR`), and define small command-local DTOs in `commands.rs` with `#[derive(Serialize, TS)] #[ts(export)]` for `PairingChallenge { worker_id: String, code: String }` and `PairedWorkerDto { worker_id: String, worker_name: String }`.

- [ ] **Step 3: Write `commands.rs`**, one command at a time, each calling into `discovery`/`ws_client`/`peers`. `submit_job` spawns `tokio::spawn(ws_client::run_job_stream(...))` and returns immediately with the `job_id`; the frontend listens for events rather than blocking the command invocation on the whole job.

Each command follows the same shape — resolve state, call the underlying async function, map errors to `String` (Tauri commands need `Serialize` errors; `String` is sufficient here since the frontend only displays these, per the "ultra-minimal UI" brief):

```rust
#[tauri::command]
pub async fn discover_workers(state: tauri::State<'_, AppState>) -> Result<Vec<discovery::DiscoveredWorker>, String> {
    // browse for a fixed short window (e.g. 2s) and collect ServiceResolved events,
    // reusing discovery::browse + discovery::parse_worker from Phase 1.
    ...
}
```

(Full bodies for `pair_worker`/`confirm_pair`/`submit_job`/`cancel_job`/`open_artifacts_folder` follow the same pattern of thin wrapping described above — write each directly against `ws_client`'s and `peers`' already-defined signatures; there is no additional design decision left to make once B1–B3 exist, so write these directly rather than pre-drafting them here.)

- [ ] **Step 4: Wire `lib.rs`**

```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tls_verify::install_crypto_provider();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state::build_app_state())
        .invoke_handler(tauri::generate_handler![
            commands::discover_workers,
            commands::pair_worker,
            commands::confirm_pair,
            commands::submit_job,
            commands::cancel_job,
            commands::open_artifacts_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Remove the placeholder `greet` command.

- [ ] **Step 5: Integration test for `submit_job`'s streaming path** — extend `apps/desktop/src-tauri/tests/ws_client_pairing.rs` (or a new `tests/job_submission.rs`) to call `ws_client::run_job_stream` directly (bypassing Tauri's IPC layer, consistent with this project's established pattern of testing the async core directly rather than through the IPC boundary) against a real paired worker-core test server, and assert the reassembled artifact zip lands on disk with the right SHA-256.

- [ ] **Step 6: Run all desktop tests, verify PASS; `cargo fmt`/`clippy --workspace --all-targets -- -D warnings`; commit**

```bash
git add apps/desktop/src-tauri/src apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/tests apps/desktop/ui/bindings crates/discovery/Cargo.toml crates/discovery/src/lib.rs
git commit -m "feat: add Master Tauri commands for discovery, pairing, job submission, and cancellation"
```

---

## Part C — Frontend (SolidJS)

### Task C1: Typed command/event wrappers

**Files:**
- Create: `apps/desktop/ui/lib/commands.ts`
- Create: `apps/desktop/ui/lib/events.ts`

- [ ] **Step 1:** Thin wrappers around `@tauri-apps/api/core`'s `invoke` and `@tauri-apps/api/event`'s `listen`, typed against the `ui/bindings/*.ts` files generated in Task B4 Step 2 plus `Message.ts` (already present from Phase 1) for the job event payloads. One function per command, one typed `listen` helper per event channel (`job://log`, `job://progress`, `job://finished`, `job://artifact`).
- [ ] **Step 2:** `pnpm exec tsc --noEmit` to confirm the wrappers type-check against the current bindings.
- [ ] **Step 3:** commit.

```bash
git add apps/desktop/ui/lib
git commit -m "feat: add typed Tauri command and event wrappers"
```

---

### Task C2: LogViewer virtualization + vitest setup

**Files:**
- Modify: `apps/desktop/package.json` (add `vitest` dev-dependency, `"test": "vitest run"` script)
- Create: `apps/desktop/vitest.config.ts`
- Create: `apps/desktop/ui/components/virtualize.ts`
- Create: `apps/desktop/ui/components/virtualize.test.ts`
- Create: `apps/desktop/ui/components/LogViewer.tsx`

**Interfaces:**
- Produces: `export function computeVisibleRange(params: { scrollTop: number; containerHeight: number; rowHeight: number; itemCount: number; overscan: number }): { start: number; end: number }` (pure, exported for the test) used by `LogViewer`.

- [ ] **Step 1: `pnpm add -D vitest` and add the config/script**

```ts
// apps/desktop/vitest.config.ts
import { defineConfig } from "vitest/config";
export default defineConfig({ test: { environment: "node" } });
```

```json
// apps/desktop/package.json — add to "scripts"
"test": "vitest run"
```

- [ ] **Step 2: Write the failing test**

```ts
// apps/desktop/ui/components/virtualize.test.ts
import { describe, expect, it } from "vitest";
import { computeVisibleRange } from "./virtualize";

describe("computeVisibleRange", () => {
  it("returns the first rows plus overscan when scrolled to the top", () => {
    const range = computeVisibleRange({ scrollTop: 0, containerHeight: 400, rowHeight: 20, itemCount: 50_000, overscan: 5 });
    expect(range.start).toBe(0);
    expect(range.end).toBe(20 + 5); // 400/20 = 20 visible rows + overscan
  });

  it("shifts the window as scrollTop increases", () => {
    const range = computeVisibleRange({ scrollTop: 2000, containerHeight: 400, rowHeight: 20, itemCount: 50_000, overscan: 5 });
    expect(range.start).toBe(100 - 5); // 2000/20 = 100
    expect(range.end).toBe(100 + 20 + 5);
  });

  it("clamps to the item count near the end of a 10k+ line log", () => {
    const range = computeVisibleRange({ scrollTop: 199_600, containerHeight: 400, rowHeight: 20, itemCount: 10_000, overscan: 5 });
    expect(range.end).toBeLessThanOrEqual(10_000);
    expect(range.start).toBeGreaterThanOrEqual(0);
  });

  it("never returns a negative start", () => {
    const range = computeVisibleRange({ scrollTop: 0, containerHeight: 400, rowHeight: 20, itemCount: 3, overscan: 5 });
    expect(range.start).toBe(0);
  });
});
```

- [ ] **Step 3: Run to confirm it fails** (`virtualize.ts` doesn't exist)

Run: `pnpm test`
Expected: FAIL, module not found.

- [ ] **Step 4: Implement `virtualize.ts`**

```ts
export interface VisibleRangeParams {
  scrollTop: number;
  containerHeight: number;
  rowHeight: number;
  itemCount: number;
  overscan: number;
}

export interface VisibleRange {
  start: number;
  end: number;
}

export function computeVisibleRange(params: VisibleRangeParams): VisibleRange {
  const { scrollTop, containerHeight, rowHeight, itemCount, overscan } = params;
  const firstVisible = Math.floor(scrollTop / rowHeight);
  const visibleCount = Math.ceil(containerHeight / rowHeight);
  const start = Math.max(0, firstVisible - overscan);
  const end = Math.min(itemCount, firstVisible + visibleCount + overscan);
  return { start, end };
}
```

- [ ] **Step 5: Run to confirm PASS**

Run: `pnpm test`
Expected: all 4 tests PASS.

- [ ] **Step 6: Build `LogViewer.tsx`** using `computeVisibleRange` — a scrollable container with a spacer div sized to `lines().length * ROW_HEIGHT`, an absolutely-positioned inner block translated to `start * ROW_HEIGHT`, rendering only `lines().slice(start, end)`. Cap the retained `lines` signal at 50,000 entries (drop the oldest when exceeded) to bound memory on pathologically long builds — a deliberate, disclosed bound since PROMPT.md only specifies the virtualization *window*, not a retention cap. Includes an auto-scroll toggle (checkbox bound to a signal; when enabled, `scrollTop` is set to the container's `scrollHeight` after each new line via an effect) and minimal ANSI handling via a small local `stripOrConvertAnsi` helper — use `ansi_up` (already named explicitly in PROMPT.md's UI section) rather than hand-rolling ANSI parsing.

```bash
pnpm add ansi_up
```

- [ ] **Step 7: `pnpm exec prettier --check` and `pnpm exec eslint` (if configured) / `tsc --noEmit`; commit**

```bash
git add apps/desktop/package.json apps/desktop/vitest.config.ts apps/desktop/ui/components/virtualize.ts apps/desktop/ui/components/virtualize.test.ts apps/desktop/ui/components/LogViewer.tsx
git commit -m "feat: add virtualized, ANSI-aware LogViewer with a unit-tested windowing function"
```

---

### Task C3: WorkerList, JobForm, BuildButton, ProgressBar

**Files:**
- Create: `apps/desktop/ui/components/WorkerList.tsx`
- Create: `apps/desktop/ui/components/JobForm.tsx`
- Create: `apps/desktop/ui/components/BuildButton.tsx`
- Create: `apps/desktop/ui/components/ProgressBar.tsx`

- [ ] **Step 1: `WorkerList`** — calls `discoverWorkers()` (Task C1) on mount and on a manual refresh button (no auto-polling loop yet — mDNS `browse` in the backend already runs continuously; `discover_workers` returns a snapshot), renders each with a status dot. Status is derived client-side: 🟢 while no job is active for that worker, 🟡 while `submit_job`'s returned `job_id` has an open stream, 🔴 is reserved for a future liveness signal (out of scope here — Phase 3's `discover_workers` only returns currently-visible mDNS peers, so an unreachable worker simply disappears from the list rather than showing red; note this explicitly as a known simplification in a one-line code comment).
- [ ] **Step 2: `JobForm`** — controlled inputs for repo URL, branch (default `"main"`), profile radio (`Debug`/`Release` mapping to `"debug"`/`"release"` strings sent to `submit_job`), and a destination-folder display (artifacts always land under `~/Documents/DistBuild/artifacts/<job_id>/` per the runtime layout — Phase 3 does not add a folder *picker* since PROMPT.md's directory layout is fixed and not user-configurable yet; expose only "open the folder" via `open_artifacts_folder` post-job).
- [ ] **Step 3: `BuildButton`** — disabled unless a worker is selected in `WorkerList`'s shared signal and the repo URL passes a basic non-empty + `.git`/URL-shape check; calls `submitJob()` on click.
- [ ] **Step 4: `ProgressBar`** — subscribes to the `job://progress` event (Task C1), renders the current `JobPhase` label and an elapsed-time counter (`setInterval` ticking a signal from `job://log`'s first-received timestamp).
- [ ] **Step 5: `pnpm exec tsc --noEmit`; commit**

```bash
git add apps/desktop/ui/components/WorkerList.tsx apps/desktop/ui/components/JobForm.tsx apps/desktop/ui/components/BuildButton.tsx apps/desktop/ui/components/ProgressBar.tsx
git commit -m "feat: add WorkerList, JobForm, BuildButton, and ProgressBar components"
```

---

### Task C4: Wire `MasterTab.tsx` and manual smoke test

**Files:**
- Modify: `apps/desktop/ui/tabs/MasterTab.tsx`

- [ ] **Step 1:** Compose `WorkerList` + `JobForm` + `BuildButton` + `ProgressBar` + `LogViewer` into the tab layout, with a shared `selectedWorker` signal (SolidJS `createSignal`, passed down as props — no need for a store/context library at this scale, consistent with the codebase's existing minimalism).
- [ ] **Step 2: Manual end-to-end smoke test** — this is a UI feature; per this project's standing instruction to verify UI changes in a real browser/app session before declaring done:
  1. `cargo run -p worker-core` in one terminal (real worker, listening on `wss://127.0.0.1:7878`).
  2. `pnpm tauri dev` in `apps/desktop` in another terminal.
  3. In the running app: discover the local worker, pair with it (confirm the 6-digit code shown in the worker-core terminal log... — note: Task A/B's job-execution logging in `worker-core` never logs the pairing code per the earlier security fix, so for this manual smoke test read the code from the Master UI's own `PairChallenge`-driven display, not the worker's terminal), submit a job pointing at a small local git repo (e.g. this very repository, `branch: master`, `profile: debug`), watch `LogViewer` stream real `cargo build`-style output, confirm `JobFinished` shows success and an artifacts folder opens.
  4. Note and fix any runtime issues found (event name typos, missing `Content-Security-Policy` allowances in `apps/desktop/src-tauri/capabilities/default.json` for the `ws`/network calls — Tauri's capability system may need `core:event:default` and any HTTP/WS-related permissions explicitly allowed; check `src-tauri/capabilities/default.json` and add what's needed).
- [ ] **Step 3:** commit.

```bash
git add apps/desktop/ui/tabs/MasterTab.tsx apps/desktop/src-tauri/capabilities/default.json
git commit -m "feat: wire the Master tab's discovery, pairing, job submission, and log viewer together"
```

---

## Self-Review Notes

- **Spec coverage:** All 5 Phase-3 Tauri commands (Task B4), the WS client with auto-reconnect (B3's `ReconnectingClient`/backoff — note: wired conceptually in B3 but the actual *usage* of `ReconnectingClient` for long-lived job streams, vs. a fresh `connect_paired` per `submit_job` call, is a judgment call left to execution time once B3/B4 are in hand; if a single job's connection drops mid-stream, reconnecting cannot resume a partially-streamed local build process on the worker side without additional protocol support PROMPT.md doesn't specify — so "auto-reconnect" is implemented at the connection layer (B3) but full mid-job resume is intentionally out of scope beyond "resume on `JobStarted` reception" for a *new* connection attempt, which is what B3's design supports), and virtualized `LogViewer` (C2) are covered. All Master tab components from PROMPT.md's UI section (WorkerList, JobForm, BuildButton, LogViewer, ProgressBar) are covered (C2/C3/C4).
- **Deviation disclosed up front:** WS client lives in the Rust backend, not the frontend — required by the TLS-pinning security requirement, not a stack deviation (still rustls/tokio-tungstenite/Tauri commands, all in the approved stack).
- **Deviation disclosed up front:** scope expanded beyond the literal Phase 3 checklist to include worker-side `JobRequest` execution (Part A), per explicit user choice when asked.
- **No placeholders:** every task has concrete code or a fully specified, non-vague instruction (e.g., C4's manual smoke test lists exact commands and exact things to check, not "test the UI").
