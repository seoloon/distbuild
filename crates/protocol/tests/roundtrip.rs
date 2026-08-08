use protocol::{JobPhase, LogStream, Message};
use std::collections::HashMap;

fn roundtrip(msg: &Message) -> Message {
    let json = serde_json::to_string(msg).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

#[test]
fn pair_request_roundtrips() {
    let msg = Message::PairRequest {
        master_name: "MacBook-Pro".into(),
        master_id: "master-abc123".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn pair_confirm_roundtrips() {
    let msg = Message::PairConfirm {
        code: "482913".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn job_request_roundtrips_with_optional_fields_present() {
    let mut env = HashMap::new();
    env.insert("RUSTFLAGS".to_string(), "-C target-cpu=native".to_string());
    let msg = Message::JobRequest {
        job_id: "job-1".into(),
        repo: "https://github.com/example/repo.git".into(),
        branch: "main".into(),
        profile: "release".into(),
        env: Some(env),
        distbuild_toml: Some("[build]\ncommand = \"make\"".into()),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn job_request_roundtrips_with_optional_fields_absent() {
    let msg = Message::JobRequest {
        job_id: "job-2".into(),
        repo: "https://github.com/example/repo.git".into(),
        branch: "main".into(),
        profile: "debug".into(),
        env: None,
        distbuild_toml: None,
    };
    let json = serde_json::to_string(&msg).expect("serialize");
    assert!(!json.contains("env"), "env should be omitted when None");
    assert!(
        !json.contains("distbuild_toml"),
        "distbuild_toml should be omitted when None"
    );
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn job_cancel_roundtrips() {
    let msg = Message::JobCancel {
        job_id: "job-1".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn pair_challenge_roundtrips() {
    let msg = Message::PairChallenge {
        code_shown_on_worker: "192837".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn pair_accepted_roundtrips() {
    let msg = Message::PairAccepted {
        worker_id: "worker-xyz".into(),
        worker_name: "Threadripper-Box".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn job_started_roundtrips() {
    let msg = Message::JobStarted {
        job_id: "job-1".into(),
        timestamp: "2026-08-08T12:00:00Z".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn log_chunk_roundtrips_and_uses_lowercase_stream_tag() {
    let msg = Message::LogChunk {
        job_id: "job-1".into(),
        stream: LogStream::Stderr,
        data: "warning: unused variable".into(),
        ts: 1_723_000_000,
    };
    let json = serde_json::to_string(&msg).expect("serialize");
    assert!(json.contains("\"stream\":\"stderr\""));
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn job_progress_roundtrips_with_and_without_pct() {
    let with_pct = Message::JobProgress {
        job_id: "job-1".into(),
        phase: JobPhase::Building,
        pct: Some(42.5),
    };
    let json = serde_json::to_string(&with_pct).expect("serialize");
    assert!(json.contains("\"phase\":\"building\""));
    assert_eq!(roundtrip(&with_pct), with_pct);

    let without_pct = Message::JobProgress {
        job_id: "job-1".into(),
        phase: JobPhase::Packaging,
        pct: None,
    };
    let json = serde_json::to_string(&without_pct).expect("serialize");
    assert!(!json.contains("pct"), "pct should be omitted when None");
    assert_eq!(roundtrip(&without_pct), without_pct);
}

#[test]
fn job_finished_roundtrips() {
    let msg = Message::JobFinished {
        job_id: "job-1".into(),
        success: true,
        duration_ms: 12_345,
        exit_code: Some(0),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn artifact_ready_roundtrips() {
    let msg = Message::ArtifactReady {
        job_id: "job-1".into(),
        filename: "artifacts.distbuild.zip".into(),
        size_bytes: 10_485_760,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn error_roundtrips() {
    let msg = Message::Error {
        code: "BUILD_FAILED".into(),
        message: "cargo build exited with status 101".into(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn tag_field_is_the_variant_name() {
    let msg = Message::JobCancel {
        job_id: "job-9".into(),
    };
    let value: serde_json::Value = serde_json::to_value(&msg).expect("to_value");
    assert_eq!(value["type"], "JobCancel");
}
