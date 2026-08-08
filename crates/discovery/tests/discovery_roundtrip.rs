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
