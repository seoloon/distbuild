use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// Every message exchanged between a Master and a Worker over the
/// DistBuild WebSocket protocol. Tagged with a `"type"` field so both
/// sides can dispatch on a single discriminant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum Message {
    // ---- Master -> Worker ----
    PairRequest {
        master_name: String,
        master_id: String,
        /// Present when the Master believes it was already paired with
        /// this worker and is reconnecting, not pairing for the first
        /// time. Must match the token the worker handed back in that
        /// prior pairing's `PairAccepted` for the worker to skip the
        /// human-confirmed code — `master_id` alone is a bare,
        /// non-secret identifier and must never be trusted as proof of
        /// identity on its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reconnect_token: Option<String>,
    },
    PairConfirm {
        code: String,
    },
    JobRequest {
        job_id: String,
        repo: String,
        branch: String,
        profile: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<HashMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        distbuild_toml: Option<String>,
    },
    JobCancel {
        job_id: String,
    },

    // ---- Worker -> Master ----
    PairChallenge {
        code_shown_on_worker: String,
    },
    PairAccepted {
        worker_id: String,
        worker_name: String,
        os: String,
        arch: String,
        /// A fresh, single-use secret the Master must present (as
        /// `PairRequest.reconnect_token`) to silently re-pair on its next
        /// connection instead of repeating the human-confirmed code.
        /// Rotated on every successful pairing, including reconnects.
        reconnect_token: String,
    },
    JobStarted {
        job_id: String,
        timestamp: String,
    },
    LogChunk {
        job_id: String,
        stream: LogStream,
        data: String,
        ts: u64,
    },
    JobProgress {
        job_id: String,
        phase: JobPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pct: Option<f32>,
    },
    JobFinished {
        job_id: String,
        success: bool,
        duration_ms: u64,
        exit_code: Option<i32>,
    },
    ArtifactReady {
        job_id: String,
        filename: String,
        size_bytes: u64,
        sha256: String,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Which output stream a [`Message::LogChunk`] was captured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Which phase of a build job a [`Message::JobProgress`] update describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum JobPhase {
    Cloning,
    Deps,
    Building,
    Packaging,
}
