//! Cross-cutting subprocess-tree lifecycle helpers.
//!
//! Vendor CLIs and audit commands may spawn descendants. On Unix we place
//! each command in a fresh process group and terminate that group before
//! returning from cancellation or timeout paths.

use tokio::process::{Child, Command};

pub(crate) fn configure(command: &mut Command) {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
}

pub(crate) async fn terminate_and_reap(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // `process_group(0)` makes the child's PID its PGID. A negative PID
        // targets the entire group, including CLI descendants.
        let group_result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        // Direct test/embedding callers may provide a Child that was not
        // created through `configure`. In that case no PGID equal to the PID
        // exists; still terminate the direct process instead of waiting for
        // natural exit.
        if group_result != 0 {
            let _ = child.start_kill();
        }
    }

    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }

    // Always reap the direct child. It may have won the exit race; that is
    // harmless and still leaves cancellation/timeout as the caller-visible
    // outcome.
    let _ = child.wait().await;
}
