//! Drives `jobs::run_job` directly (not through the socket layer — that's
//! covered by `job_over_websocket.rs`) against a real local git fixture,
//! proving clone -> detect -> build -> package -> chunk actually works.

use std::fs;
use std::path::Path;

use protocol::{JobPhase, Message};
use tokio::sync::mpsc;
use worker_core::jobs::{run_job, JobEvent, JobHandle, JobParams};

fn init_fixture_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).expect("init fixture repo");
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .expect("write");
    fs::create_dir_all(dir.join("src")).expect("mkdir");
    fs::write(
        dir.join("src").join("main.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .expect("write");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .expect("add_all");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("write_tree"))
        .expect("find_tree");
    let sig = git2::Signature::now("Fixture", "fixture@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .expect("commit");
    let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
    if branch_name != "main" {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("main", &commit, true).expect("branch");
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
            JobEvent::Text(Message::JobFinished {
                success, job_id, ..
            }) => {
                assert_eq!(job_id, "job-e2e-1");
                assert!(success, "the fixture build should succeed");
                saw_finished = true;
            }
            JobEvent::Text(Message::ArtifactReady {
                job_id, size_bytes, ..
            }) => {
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
    assert!(
        binary_frames >= 1,
        "artifact should be sent as at least one binary frame"
    );
    assert_eq!(
        phases,
        vec![JobPhase::Cloning, JobPhase::Building, JobPhase::Packaging]
    );

    assert!(runtime_dir
        .path()
        .join("jobs")
        .join("job-e2e-1")
        .join("manifest.json")
        .is_file());
    assert!(runtime_dir
        .path()
        .join("jobs")
        .join("job-e2e-1")
        .join("stdout.log")
        .is_file());
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

    run_job(params, runtime_dir.path(), tx, handle)
        .await
        .expect("run_job");

    let mut saw_finished_unsuccessfully = false;
    while let Ok(event) = rx.try_recv() {
        if let JobEvent::Text(Message::JobFinished { success, .. }) = event {
            assert!(!success);
            saw_finished_unsuccessfully = true;
        }
    }
    assert!(saw_finished_unsuccessfully);
}
