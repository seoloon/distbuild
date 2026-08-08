//! Binary artifact-chunk framing sent after `Message::ArtifactReady`:
//! each WebSocket *binary* frame is
//! `[4 bytes: header_len as u32 little-endian][header_len bytes: JSON-encoded artifacts::ChunkHeader][remaining bytes: chunk payload]`.
//! The Master reads the u32, decodes the header, then treats the rest of
//! the frame as raw chunk bytes — this keeps chunk metadata self-describing
//! per-frame without needing a separate control channel.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use artifacts::{chunk_file, collect_artifacts, default_globs, package_zip, ArtifactKind};
use executor::{clone_repo, detect_build_system, run_step, BuildSystem};
use protocol::{JobPhase, Message};
use serde::Serialize;
use tokio::sync::mpsc;

/// One outbound item produced while a job runs: either a protocol message
/// (sent as a WS text frame) or a raw artifact-chunk frame (sent as a WS
/// binary frame, framed per the module doc comment above).
pub enum JobEvent {
    Text(Message),
    Binary(Vec<u8>),
}

/// Cooperative cancellation flag for one running job, checked between
/// build steps. The `/ws` handler calls `.cancel()` in response to
/// `Message::JobCancel`.
#[derive(Clone, Default)]
pub struct JobHandle {
    cancelled: Arc<AtomicBool>,
}

impl JobHandle {
    pub fn new() -> Self {
        Self::default()
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

#[derive(Debug, Serialize)]
struct Manifest<'a> {
    job_id: &'a str,
    repo: &'a str,
    branch: &'a str,
    profile: &'a str,
    success: bool,
    duration_ms: u64,
    exit_code: Option<i32>,
}

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
        timestamp: unix_millis().to_string(),
    }));

    if handle.is_cancelled() {
        finish(&out, &job_dir, &params, false, started_at, None)?;
        return Ok(());
    }

    let _ = out.send(JobEvent::Text(Message::JobProgress {
        job_id: params.job_id.clone(),
        phase: JobPhase::Cloning,
        pct: None,
    }));
    if clone_repo(&params.repo, &params.branch, &repo_dir)
        .await
        .is_err()
    {
        finish(&out, &job_dir, &params, false, started_at, None)?;
        return Ok(());
    }

    let Some(build_system) = detect_build_system(&repo_dir) else {
        finish(&out, &job_dir, &params, false, started_at, None)?;
        return Ok(());
    };

    let mut stdout_log =
        std::io::BufWriter::new(std::fs::File::create(job_dir.join("stdout.log"))?);
    let mut stderr_log =
        std::io::BufWriter::new(std::fs::File::create(job_dir.join("stderr.log"))?);

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
            let (log_tx, mut log_rx) = mpsc::unbounded_channel::<executor::LogLine>();
            let job_id = params.job_id.clone();
            let out_clone = out.clone();
            let forward_task = tokio::spawn(async move {
                while let Some(line) = log_rx.recv().await {
                    match line.stream {
                        protocol::LogStream::Stdout => {
                            let _ = writeln!(stdout_log, "{}", line.data);
                        }
                        protocol::LogStream::Stderr => {
                            let _ = writeln!(stderr_log, "{}", line.data);
                        }
                    }
                    let _ = out_clone.send(JobEvent::Text(Message::LogChunk {
                        job_id: job_id.clone(),
                        stream: line.stream,
                        data: line.data,
                        ts: unix_millis(),
                    }));
                }
                (stdout_log, stderr_log)
            });

            let result = run_step(&program, &args, &repo_dir, &log_tx).await;
            drop(log_tx);
            let (returned_stdout, returned_stderr) =
                forward_task.await.expect("log forwarding task panicked");
            stdout_log = returned_stdout;
            stderr_log = returned_stderr;

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

    let _ = stdout_log.flush();
    let _ = stderr_log.flush();

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
            let binary_name = cargo_package_name(&repo_dir).unwrap_or_else(|| {
                repo_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let globs = default_globs(kind, &params.profile, &binary_name);
            match collect_artifacts(&repo_dir, &globs) {
                Ok(files) if !files.is_empty() => {
                    let zip_path = job_dir.join("artifacts.distbuild.zip");
                    match package_zip(&files, &repo_dir, &zip_path) {
                        Ok(summary) => {
                            let _ = out.send(JobEvent::Text(Message::ArtifactReady {
                                job_id: params.job_id.clone(),
                                filename: "artifacts.distbuild.zip".to_string(),
                                size_bytes: summary.size_bytes,
                                sha256: summary.sha256,
                            }));
                            match chunk_file(&zip_path, &params.job_id, 256 * 1024) {
                                Ok(chunks) => {
                                    for (header, bytes) in chunks {
                                        let _ = out
                                            .send(JobEvent::Binary(frame_chunk(&header, &bytes)));
                                    }
                                }
                                Err(error) => {
                                    tracing::error!(%error, job_id = %params.job_id, "failed to chunk the packaged artifact")
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, job_id = %params.job_id, "failed to package artifacts into a zip")
                        }
                    }
                }
                Ok(_) => {
                    tracing::warn!(job_id = %params.job_id, ?globs, "build succeeded but no artifacts matched the expected globs")
                }
                Err(error) => {
                    tracing::error!(%error, job_id = %params.job_id, "failed to collect artifacts")
                }
            }
        }
    }

    finish(
        &out,
        &job_dir,
        &params,
        all_succeeded && !handle.is_cancelled(),
        started_at,
        last_exit_code,
    )?;
    Ok(())
}

fn finish(
    out: &mpsc::UnboundedSender<JobEvent>,
    job_dir: &Path,
    params: &JobParams,
    success: bool,
    started_at: Instant,
    exit_code: Option<i32>,
) -> std::io::Result<()> {
    let duration_ms = started_at.elapsed().as_millis() as u64;

    let manifest = Manifest {
        job_id: &params.job_id,
        repo: &params.repo,
        branch: &params.branch,
        profile: &params.profile,
        success,
        duration_ms,
        exit_code,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).expect("Manifest serialization cannot fail");
    std::fs::write(job_dir.join("manifest.json"), manifest_json)?;

    let _ = out.send(JobEvent::Text(Message::JobFinished {
        job_id: params.job_id.clone(),
        success,
        duration_ms,
        exit_code,
    }));
    Ok(())
}

/// Reads `[package].name` out of the cloned repo's `Cargo.toml`, since the
/// compiled binary is named after the Cargo package, not the checkout
/// directory (which is always literally `repo`, from `run_job`'s own
/// `job_dir.join("repo")` layout).
fn cargo_package_name(repo_dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(repo_dir.join("Cargo.toml")).ok()?;
    let table: toml::Table = contents.parse().ok()?;
    table
        .get("package")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
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
