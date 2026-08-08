# DistBuild Phase 1 — Protocol + Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `protocol` crate (shared `Message` enum + TS bindings) and the `discovery` crate (mDNS broadcast/browse), each with the tests required by `claude/PROMPT.md`'s Phase 1 line items 5–7.

**Architecture:** `protocol` is a pure data crate: one `#[serde(tag = "type")]` enum covering every Master↔Worker message, plus two small sub-enums (`LogStream`, `JobPhase`). It derives `ts-rs::TS` so the SolidJS frontend can consume generated `.ts` files instead of hand-written duplicates (non-negotiable rule 5). `discovery` wraps the `mdns-sd` crate: `broadcast()` registers a worker under `_distbuild._tcp.local.` with TXT records, `browse()` returns the daemon's event receiver, and `parse_worker()` decodes a resolved service back into a typed `DiscoveredWorker`. Both crates ship with tests proving round-trip correctness end-to-end (JSON round-trip for protocol, live localhost mDNS round-trip for discovery), matching PROMPT.md's explicit "Integration test: two processes on localhost see each other in < 1s" requirement.

**Tech Stack:** Rust, `serde`/`serde_json` (workspace deps), `ts-rs` 12.0.1, `mdns-sd` 0.20.3 (`async` feature), `thiserror` (workspace dep), `tracing` (workspace dep).

## Global Constraints

- No `.unwrap()` outside tests; no `panic!` in library code (non-negotiable rule 7).
- All errors go through `thiserror`-derived enums (non-negotiable rule 7).
- `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` must be clean before each commit (non-negotiable rule 8).
- Commit messages follow Conventional Commits (non-negotiable rule 9); create new commits, never amend.
- No hand-written TypeScript duplicates of Rust protocol types — generate via `ts-rs` (non-negotiable rule 5).
- Work happens directly on `master` (established in Phase 0), inline execution (established in Phase 0).

## Already Done (prerequisite research, not a task)

While drafting this plan I added the real dependencies so the code below is verified against the actual installed crate APIs (not guessed):

- `crates/protocol/Cargo.toml`: added `serde = { workspace = true, features = ["derive"] }`, `serde_json.workspace = true`, `ts-rs = "12.0.1"`.
- `crates/discovery/Cargo.toml`: added `mdns-sd = { version = "0.20.3", features = ["async"] }`, `thiserror.workspace = true`, `tracing.workspace = true`.
- Verified `cargo build -p discovery -p protocol` succeeds with these deps.
- Read the vendored `mdns-sd` 0.20.3 source (`ServiceInfo::new`, `ServiceDaemon::browse`, `ServiceEvent`, `ResolvedService`, `TxtProperties::get_property_val_str`, `ScopedIp::to_ip_addr`) and `ts-rs` 12.0.1 source (serde-compat attribute support, `TS_RS_EXPORT_DIR` env var) to confirm every signature used in the tasks below.

---

### Task 1: Protocol crate — `Message` enum + TS export wiring

**Files:**
- Create: `.cargo/config.toml` (workspace root — sets `TS_RS_EXPORT_DIR`)
- Modify: `crates/protocol/src/lib.rs`

**Interfaces:**
- Produces: `protocol::Message` (enum, `Serialize + Deserialize + Debug + Clone + PartialEq`, `#[serde(tag = "type")]`), `protocol::LogStream` (`Stdout | Stderr`), `protocol::JobPhase` (`Cloning | Deps | Building | Packaging`). Later tasks (Task 2 tests, and Phase 2's Axum `/ws` handler) construct/match on these directly.

- [ ] **Step 1: Point `ts-rs` exports at the frontend bindings folder**

Create `.cargo/config.toml` at the workspace root:

```toml
[env]
TS_RS_EXPORT_DIR = { value = "apps/desktop/ui/bindings", relative = true }
```

- [ ] **Step 2: Write the `Message` enum and sub-enums**

Replace the contents of `crates/protocol/src/lib.rs`:

```rust
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
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p protocol`
Expected: clean compile, no warnings.

- [ ] **Step 4: Commit**

```bash
git add .cargo/config.toml crates/protocol/Cargo.toml crates/protocol/src/lib.rs
git commit -m "feat: define protocol Message enum with ts-rs export wiring"
```

---

### Task 2: Protocol crate — round-trip tests + TS binding generation

**Files:**
- Create: `crates/protocol/tests/roundtrip.rs`

**Interfaces:**
- Consumes: `protocol::{Message, LogStream, JobPhase}` from Task 1.

- [ ] **Step 1: Write round-trip tests for every variant**

Create `crates/protocol/tests/roundtrip.rs`:

```rust
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
    let msg = Message::PairConfirm { code: "482913".into() };
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
    let msg = Message::JobCancel { job_id: "job-1".into() };
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
    let msg = Message::JobCancel { job_id: "job-9".into() };
    let value: serde_json::Value = serde_json::to_value(&msg).expect("to_value");
    assert_eq!(value["type"], "JobCancel");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p protocol`
Expected: all tests pass, including the `ts-rs`-generated `export_bindings_*` tests (one per `#[ts(export)]` type), which write to `apps/desktop/ui/bindings/`.

- [ ] **Step 3: Confirm TS bindings were generated**

Run: `ls apps/desktop/ui/bindings`
Expected: `Message.ts`, `LogStream.ts`, `JobPhase.ts` present.

- [ ] **Step 4: Commit**

```bash
git add crates/protocol/tests/roundtrip.rs apps/desktop/ui/bindings
git commit -m "test: add protocol Message roundtrip tests and generated TS bindings"
```

---

### Task 3: Discovery crate — `broadcast`, `browse`, `parse_worker`

**Files:**
- Modify: `crates/discovery/src/lib.rs`

**Interfaces:**
- Produces: `discovery::SERVICE_TYPE: &str`, `discovery::WorkerAnnounce { name, os, arch, port, version }`, `discovery::DiscoveredWorker { name, os, arch, port, version, addresses: Vec<IpAddr> }`, `discovery::DiscoveryError`, `discovery::broadcast(&WorkerAnnounce) -> Result<ServiceDaemon, DiscoveryError>`, `discovery::browse(&ServiceDaemon) -> Result<Receiver<ServiceEvent>, DiscoveryError>`, `discovery::parse_worker(&ResolvedService) -> Option<DiscoveredWorker>`. Task 4's test, and Phase 3's Master-side `discover_workers` Tauri command, consume these directly. Re-exports `mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent}` so downstream crates don't need a direct `mdns-sd` dependency.

- [ ] **Step 1: Write the broadcast/browse/parse API**

Replace the contents of `crates/discovery/src/lib.rs`:

```rust
use std::collections::HashMap;
use std::net::IpAddr;

use thiserror::Error;

pub use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent};

/// mDNS service type DistBuild workers advertise under.
pub const SERVICE_TYPE: &str = "_distbuild._tcp.local.";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mDNS daemon error: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

/// Information a Worker broadcasts about itself over mDNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAnnounce {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub port: u16,
    pub version: String,
}

/// A worker discovered by browsing, decoded from mDNS TXT records.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredWorker {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub port: u16,
    pub version: String,
    pub addresses: Vec<IpAddr>,
}

/// Registers a Worker's mDNS service and starts responding to browsers on
/// [`SERVICE_TYPE`]. The returned [`ServiceDaemon`] must be kept alive for
/// as long as the service should stay advertised; dropping it (or calling
/// `.shutdown()`) stops the broadcast.
pub fn broadcast(announce: &WorkerAnnounce) -> Result<ServiceDaemon, DiscoveryError> {
    let daemon = ServiceDaemon::new()?;
    let host_name = format!("{}.local.", announce.name);

    let mut properties = HashMap::new();
    properties.insert("name".to_string(), announce.name.clone());
    properties.insert("os".to_string(), announce.os.clone());
    properties.insert("arch".to_string(), announce.arch.clone());
    properties.insert("port".to_string(), announce.port.to_string());
    properties.insert("version".to_string(), announce.version.clone());

    let service_info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &announce.name,
        &host_name,
        "",
        announce.port,
        properties,
    )?
    .enable_addr_auto();

    daemon.register(service_info)?;
    Ok(daemon)
}

/// Starts browsing for DistBuild workers on the LAN. Each event on the
/// returned receiver is a discovery update; decode `ServiceResolved` events
/// with [`parse_worker`].
pub fn browse(daemon: &ServiceDaemon) -> Result<Receiver<ServiceEvent>, DiscoveryError> {
    Ok(daemon.browse(SERVICE_TYPE)?)
}

/// Decodes a resolved mDNS service into a [`DiscoveredWorker`], if it
/// carries all the TXT properties DistBuild workers publish.
pub fn parse_worker(resolved: &ResolvedService) -> Option<DiscoveredWorker> {
    let props = &resolved.txt_properties;
    Some(DiscoveredWorker {
        name: props.get_property_val_str("name")?.to_string(),
        os: props.get_property_val_str("os")?.to_string(),
        arch: props.get_property_val_str("arch")?.to_string(),
        version: props.get_property_val_str("version")?.to_string(),
        port: resolved.port,
        addresses: resolved.addresses.iter().map(|a| a.to_ip_addr()).collect(),
    })
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p discovery`
Expected: clean compile, no warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/discovery/Cargo.toml crates/discovery/src/lib.rs
git commit -m "feat: implement discovery crate with mDNS broadcast/browse/parse"
```

---

### Task 4: Discovery crate — localhost round-trip integration test

**Files:**
- Create: `crates/discovery/tests/discovery_roundtrip.rs`

**Interfaces:**
- Consumes: `discovery::{broadcast, browse, parse_worker, WorkerAnnounce, ServiceEvent}` from Task 3.

- [ ] **Step 1: Write the integration test**

Create `crates/discovery/tests/discovery_roundtrip.rs`:

```rust
use discovery::{broadcast, browse, parse_worker, ServiceEvent, WorkerAnnounce};
use std::time::{Duration, Instant};

/// PROMPT.md Phase 1 requirement: "Integration test: two processes on
/// localhost see each other in < 1s". We simulate the two processes as two
/// independent mDNS daemons within one test process (broadcaster + browser),
/// which exercises the same wire protocol two real processes would use.
#[test]
fn discovers_worker_on_localhost_within_one_second() {
    let announce = WorkerAnnounce {
        name: format!("test-worker-{}", std::process::id()),
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        port: 9876,
        version: "0.1.0".to_string(),
    };

    let worker_daemon = broadcast(&announce).expect("failed to broadcast worker service");

    let browser_daemon = mdns_sd::ServiceDaemon::new().expect("failed to create browser daemon");
    let receiver = browse(&browser_daemon).expect("failed to start browsing");

    let deadline = Instant::now() + Duration::from_secs(1);
    let mut found = None;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(resolved)) => {
                if let Some(worker) = parse_worker(&resolved) {
                    if worker.name == announce.name {
                        found = Some(worker);
                        break;
                    }
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let worker = found.expect("worker was not discovered on localhost within 1 second");
    assert_eq!(worker.os, "linux");
    assert_eq!(worker.arch, "x86_64");
    assert_eq!(worker.port, 9876);
    assert_eq!(worker.version, "0.1.0");
    assert!(
        !worker.addresses.is_empty(),
        "resolved worker should have at least one address"
    );

    let _ = browser_daemon.shutdown();
    let _ = worker_daemon.shutdown();
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p discovery -- --nocapture`
Expected: `discovers_worker_on_localhost_within_one_second` passes. This test uses real multicast on loopback — if it fails in a sandboxed/CI environment with multicast disabled, re-run with `RUST_LOG=mdns_sd=debug` to check whether packets are leaving the loopback interface at all before treating it as a code bug.

- [ ] **Step 3: Commit**

```bash
git add crates/discovery/tests/discovery_roundtrip.rs
git commit -m "test: add discovery localhost roundtrip integration test"
```

---

### Task 5: Workspace-wide verification

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Expected: no diff (or only whitespace changes to the files just written — re-stage if so).

- [ ] **Step 2: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean, zero warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: all tests pass, including `protocol`'s roundtrip + ts-rs export tests, `discovery`'s roundtrip integration test, and the placeholder `it_works` tests in the untouched crates.

- [ ] **Step 4: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore: fmt fixes after phase 1 implementation"
```

(Skip this step if `cargo fmt --all` produced no diff.)

---

## Definition of Done

- `protocol` crate exposes `Message`/`LogStream`/`JobPhase` matching PROMPT.md's protocol section exactly, with passing round-trip tests for all 12 variants.
- `ts-rs` bindings are generated into `apps/desktop/ui/bindings/` (no hand-written TS duplicates).
- `discovery` crate broadcasts under `_distbuild._tcp.local.` with `{name, os, arch, port, version}` TXT records and browses for the same.
- The localhost discovery integration test passes in under 1 second, satisfying PROMPT.md's explicit Phase 1 exit criterion.
- `cargo fmt --all`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` are all clean.
- All work committed directly to `master` as separate, Conventional-Commits-style commits (no amending).
- **Stop here.** Do not begin Phase 2 (Worker daemon / Axum `/ws` handler / pairing) without the user's explicit go-ahead, per PROMPT.md's phase-gate instruction.
