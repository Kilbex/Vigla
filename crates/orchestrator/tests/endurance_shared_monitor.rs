//! U10 A6: the process-level shared-monitor install/read seam the host
//! uses to inject a heartbeat into every mission.
//!
//! This lives in its own integration binary on purpose: installing the
//! process-wide `OnceLock` is irreversible, so it must not leak into the
//! lib test binary (whose mission tests assume no monitor is installed).

use orchestrator::endurance::{heartbeat_path, install_process_monitor, shared_monitor};
use tempfile::TempDir;

#[test]
fn install_makes_the_monitor_globally_readable_and_is_set_once() {
    // Fresh binary: nothing installed yet.
    assert!(shared_monitor().is_none(), "no monitor before install");

    let dir = TempDir::new().unwrap();
    install_process_monitor(dir.path()).expect("launch + install");

    // Now globally readable, and `launch` wrote the initial heartbeat.
    assert!(shared_monitor().is_some(), "monitor readable after install");
    assert!(
        heartbeat_path(dir.path()).exists(),
        "launch wrote the durable heartbeat"
    );

    // Set-once: a second install at a different root is a no-op (first
    // writer wins) and must not even launch a second monitor.
    let dir2 = TempDir::new().unwrap();
    install_process_monitor(dir2.path()).expect("second install is a no-op");
    assert!(
        !heartbeat_path(dir2.path()).exists(),
        "second install must not launch — first writer wins"
    );
}
