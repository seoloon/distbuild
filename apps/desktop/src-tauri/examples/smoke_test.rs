//! Manual Phase 3 smoke test (Task C4): drives the real Master-side Rust
//! logic (pairing, TLS pinning, job submission, artifact reassembly)
//! against a genuinely separate `worker-core.exe` OS process listening on
//! 127.0.0.1:7878 — not an in-process test server like the automated
//! integration tests use. This is as close to "click through the real UI"
//! as is achievable without an attached display; it exercises the exact
//! same `desktop_lib::{ws_client, peers, tls_verify}` functions the Tauri
//! commands call.
//!
//! Usage: `cargo run --example smoke_test -p desktop`, with a real
//! worker-core.exe already running and listening on 127.0.0.1:7878.

use std::fs;
use std::path::Path;

use desktop_lib::ws_client::{self, JobEventSink, JobRequestArgs};
use protocol::Message;
use tokio_util::sync::CancellationToken;

struct PrintSink;
impl JobEventSink for PrintSink {
    fn emit(&self, event: &str, message: &Message) {
        println!("[{event}] {message:?}");
    }
}

fn init_fixture_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).expect("init fixture repo");
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"smoke-fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .expect("write Cargo.toml");
    fs::create_dir_all(dir.join("src")).expect("mkdir src");
    fs::write(
        dir.join("src").join("main.rs"),
        "fn main() { println!(\"hello from the smoke-test fixture\"); }\n",
    )
    .expect("write main.rs");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .expect("add_all");
    index.write().expect("write index");
    let tree = repo
        .find_tree(index.write_tree().expect("write_tree"))
        .expect("find_tree");
    let sig = git2::Signature::now("Smoke Test", "smoke@example.com").expect("sig");
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .expect("commit");
    let branch_name = repo.head().unwrap().shorthand().unwrap().to_string();
    if branch_name != "main" {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("main", &commit, true).expect("branch");
        repo.set_head("refs/heads/main").expect("set_head");
    }
}

#[tokio::main]
async fn main() {
    desktop_lib::tls_verify::install_crypto_provider();

    let host = "127.0.0.1";
    let port = 7878u16;

    println!("== discover (skipped — connecting directly to {host}:{port}) ==");

    println!("== pair_worker: begin_pairing ==");
    let session = ws_client::begin_pairing(host, port, "SmokeTest-Master", "master-smoke-test")
        .await
        .expect("begin_pairing failed — is worker-core.exe running on 127.0.0.1:7878?");
    println!("PairChallenge code: {}", session.code);
    let fingerprint = session.fingerprint;

    println!("== confirm_pair ==");
    let code = session.code.clone();
    let mut peer = ws_client::confirm_pairing(session, code)
        .await
        .expect("confirm_pairing failed");
    peer.host = host.to_string();
    peer.port = port;
    println!(
        "Paired: worker_id={} worker_name={} fingerprint={}",
        peer.worker_id,
        peer.worker_name,
        hex::encode(fingerprint)
    );

    println!("== submit_job: cloning a fixture repo and building it on the real worker ==");
    let src = tempfile::tempdir().expect("tempdir");
    init_fixture_repo(src.path());

    let (stream, rotated_token) =
        ws_client::resume_paired_connection(&peer, "SmokeTest-Master", "master-smoke-test")
            .await
            .expect("resume_paired_connection failed");
    println!("Reconnected with rotated token: {}", &rotated_token[..8]);

    let artifacts_dir = std::env::temp_dir().join("distbuild-smoke-test-artifacts");
    let job_id = "smoke-test-job-1".to_string();
    let job_args = JobRequestArgs {
        job_id: job_id.clone(),
        repo: src.path().to_string_lossy().into_owned(),
        branch: "main".to_string(),
        profile: "debug".to_string(),
    };

    ws_client::run_job_stream(
        stream,
        job_args,
        PrintSink,
        artifacts_dir.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("run_job_stream failed");

    let job_dir = artifacts_dir.join(&job_id);
    match fs::read_dir(&job_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let meta = entry.metadata().expect("metadata");
                println!(
                    "== artifact written: {} ({} bytes) ==",
                    entry.path().display(),
                    meta.len()
                );
            }
        }
        Err(error) => println!("== no artifact directory found: {error} =="),
    }

    println!("== smoke test complete ==");
}
