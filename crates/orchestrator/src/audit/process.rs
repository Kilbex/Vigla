//! Timeout-safe subprocess execution for audit tools.
//!
//! Audit commands frequently spawn descendants (`cargo` → test binaries,
//! `npm` → a test runner). Killing only the direct child on timeout leaves
//! those descendants running against a worktree that may immediately be
//! removed. On Unix, every command therefore gets its own process group and
//! the complete group is terminated on timeout.

use std::collections::VecDeque;
use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub(super) enum TimedCommandError {
    Spawn(io::Error),
    Timeout(Duration),
}

/// A test/build tool can produce gigabytes before its timeout. Keep enough
/// context to diagnose both startup and the final failure summary without
/// letting captured pipes grow with child output.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const TRUNCATION_MARKER: &[u8] = b"\n[... output truncated by Vigla ...]\n";

pub(super) async fn output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<Output, TimedCommandError> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(TimedCommandError::Spawn)?;
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(TimedCommandError::Spawn)?,
        Err(_) => {
            terminate_tree(&mut child);
            // Reap the direct child so it cannot become a zombie. Ignore a
            // concurrent-exit error: timeout is still the caller-visible fact.
            let _ = child.wait().await;
            let _ = collect_pipe(stdout).await;
            let _ = collect_pipe(stderr).await;
            return Err(TimedCommandError::Timeout(timeout));
        }
    };

    let stdout = collect_pipe(stdout)
        .await
        .map_err(TimedCommandError::Spawn)?;
    let stderr = collect_pipe(stderr)
        .await
        .map_err(TimedCommandError::Spawn)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_pipe<R>(mut pipe: R) -> JoinHandle<io::Result<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let head_limit = (MAX_CAPTURE_BYTES - TRUNCATION_MARKER.len()) / 2;
        let tail_limit = MAX_CAPTURE_BYTES - TRUNCATION_MARKER.len() - head_limit;
        let mut head = Vec::with_capacity(head_limit);
        let mut tail: VecDeque<u8> = VecDeque::with_capacity(tail_limit);
        let mut total = 0usize;
        let mut chunk = [0u8; 8192];

        loop {
            let read = pipe.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read);
            let mut offset = 0;
            if head.len() < head_limit {
                let take = (head_limit - head.len()).min(read);
                head.extend_from_slice(&chunk[..take]);
                offset = take;
            }
            if offset < read {
                tail.extend(chunk[offset..read].iter().copied());
                if tail.len() > tail_limit {
                    tail.drain(..tail.len() - tail_limit);
                }
            }
        }

        let mut bytes = Vec::with_capacity(total.min(MAX_CAPTURE_BYTES));
        bytes.extend_from_slice(&head);
        if total > MAX_CAPTURE_BYTES {
            bytes.extend_from_slice(TRUNCATION_MARKER);
        }
        bytes.extend(tail);
        Ok(bytes)
    })
}

async fn collect_pipe(task: Option<JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match task {
        Some(task) => task
            .await
            .map_err(|error| io::Error::other(format!("pipe reader task failed: {error}")))?,
        None => Ok(Vec::new()),
    }
}

#[cfg(unix)]
fn terminate_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // `process_group(0)` made the child's PID its PGID. A negative PID
        // addresses that group, including cargo/npm descendants. SIGKILL is
        // appropriate after the hard timeout; the worktree must be quiescent
        // before the audit returns.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn terminate_tree(child: &mut tokio::process::Child) {
    // The current desktop and portable CI targets are Unix. Retain a safe
    // direct-child fallback for other targets until a Job Object backend is
    // added with the Windows port.
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn captures_stdout_and_stderr() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("printf out; printf err >&2");
        let output = output_with_timeout(&mut command, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
    }

    #[tokio::test]
    async fn pipe_capture_is_bounded_and_preserves_head_and_tail() {
        use tokio::io::AsyncWriteExt;

        let (reader, mut writer) = tokio::io::duplex(16 * 1024);
        let write = tokio::spawn(async move {
            writer.write_all(b"BEGIN").await.unwrap();
            writer
                .write_all(&vec![b'x'; MAX_CAPTURE_BYTES * 2])
                .await
                .unwrap();
            writer.write_all(b"END").await.unwrap();
        });
        let captured = collect_pipe(Some(read_pipe(reader))).await.unwrap();
        write.await.unwrap();

        assert_eq!(captured.len(), MAX_CAPTURE_BYTES);
        assert!(captured.starts_with(b"BEGIN"));
        assert!(captured.ends_with(b"END"));
        assert!(captured
            .windows(TRUNCATION_MARKER.len())
            .any(|window| window == TRUNCATION_MARKER));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_descendants_before_returning() {
        let dir = tempdir().unwrap();
        let marker = dir.path().join("orphan-survived");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("(sleep 0.25; printf survived > \"$1\") & wait")
            .arg("vigla-audit-timeout-test")
            .arg(&marker);

        let result = output_with_timeout(&mut command, Duration::from_millis(50)).await;
        assert!(matches!(result, Err(TimedCommandError::Timeout(_))));
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !marker.exists(),
            "a timed-out audit left a descendant running"
        );
    }
}
