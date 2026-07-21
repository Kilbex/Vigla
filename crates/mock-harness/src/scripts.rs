//! Script trajectories. Each public function returns a complete
//! [`TimedEvent`] sequence for one [`crate::Script`]. Sequences are
//! built via the private [`Builder`] helper that handles seq/ts
//! bookkeeping so each script body reads as a stage-by-stage script.

use crate::time::rfc3339_from_unix_ms;
use crate::{EmitOpts, TimedEvent};
use event_schema::{
    Artifact, ArtifactKind, Completion, Cost, Dependency, Event, EventKind, Failure,
    FailureCategory, FileActivity, FileOp, Log, LogLevel, LogStream, Progress, StateChange,
    TestFailure, TestResult, WorkerState, SCHEMA_VERSION,
};

struct Builder<'a> {
    opts: &'a EmitOpts,
    out: Vec<TimedEvent>,
    seq: u64,
    cumulative_ms: u64,
}

impl<'a> Builder<'a> {
    fn new(opts: &'a EmitOpts) -> Self {
        Self {
            opts,
            out: Vec::new(),
            seq: 0,
            cumulative_ms: 0,
        }
    }

    fn push(&mut self, delta_ms: u64, task_id: Option<String>, kind: EventKind) {
        self.cumulative_ms += delta_ms;
        let ts = rfc3339_from_unix_ms(self.opts.start_unix_ms + self.cumulative_ms);
        let event = Event {
            schema_version: SCHEMA_VERSION.to_string(),
            worker_id: self.opts.worker_id.clone(),
            task_id,
            seq: self.seq,
            ts,
            kind,
        };
        self.seq += 1;
        self.out.push(TimedEvent {
            event,
            delay_ms_before: delta_ms,
        });
    }

    /// Emit at the worker level (`task_id = null`). Used for the
    /// initial `idle` announcement every adapter emits first.
    fn worker(&mut self, delta_ms: u64, kind: EventKind) {
        self.push(delta_ms, None, kind);
    }

    /// Emit attached to the configured task.
    fn task(&mut self, delta_ms: u64, kind: EventKind) {
        let id = self.opts.task_id.clone();
        self.push(delta_ms, Some(id), kind);
    }

    /// Render a future ts string at the next emission point. Useful for
    /// the `Dependency.since` field where we want a timestamp that
    /// matches the dependency event's own ts.
    fn future_ts(&self, delta_ms: u64) -> String {
        rfc3339_from_unix_ms(self.opts.start_unix_ms + self.cumulative_ms + delta_ms)
    }

    fn finish(self) -> Vec<TimedEvent> {
        self.out
    }
}

// ---------------------------------------------------------------------
// claude_happy: planning → executing → reviewing → done
// ---------------------------------------------------------------------

pub fn claude_happy(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    // Initial worker-level idle (per §5 producer obligation #1).
    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );

    // Plan
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Planning,
            from: Some(WorkerState::Idle),
            note: Some("drafting plan".into()),
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 10.0,
            eta_ms: Some(3000),
            note: Some("decomposed task".into()),
        }),
    );
    b.task(
        300,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "drafted plan: 4 steps".into(),
            tag: Some("plan".into()),
        }),
    );

    // Execute
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Planning),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 25.0,
            eta_ms: Some(2700),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::FileActivity(FileActivity {
            path: "src/fetcher.ts".into(),
            op: FileOp::Created,
            from_path: None,
            lines_added: Some(48),
            lines_removed: None,
            bytes: None,
        }),
    );
    b.task(
        300,
        EventKind::Progress(Progress {
            percent: 50.0,
            eta_ms: Some(1800),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::FileActivity(FileActivity {
            path: "src/fetcher.test.ts".into(),
            op: FileOp::Created,
            from_path: None,
            lines_added: Some(22),
            lines_removed: None,
            bytes: None,
        }),
    );
    b.task(
        200,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "patch applied".into(),
            tag: None,
        }),
    );
    b.task(
        300,
        EventKind::Progress(Progress {
            percent: 75.0,
            eta_ms: Some(900),
            note: None,
        }),
    );

    // Review
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Reviewing,
            from: Some(WorkerState::Executing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::TestResult(TestResult {
            suite: "vitest".into(),
            passed: 19,
            failed: 0,
            skipped: 0,
            duration_ms: 1230,
            failures: None,
        }),
    );
    b.task(
        200,
        EventKind::Cost(Cost {
            input_tokens: 4210,
            output_tokens: 980,
            usd: 0.0186,
            cache_read_tokens: Some(11_200),
            cache_write_tokens: Some(0),
            model: Some("claude-opus-4-7".into()),
        }),
    );

    // Done
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Done,
            from: Some(WorkerState::Reviewing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Completion(Completion {
            summary: "Implemented retry-with-backoff in fetcher; 1 unit test added; 19 tests pass."
                .into(),
            artifacts: Some(vec![
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/fetcher.ts".into(),
                    label: Some("created".into()),
                },
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/fetcher.test.ts".into(),
                    label: Some("+1 test".into()),
                },
            ]),
            duration_ms: Some(3500),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// codex_blocked: executing → blocked → executing → done
// ---------------------------------------------------------------------

pub fn codex_blocked(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );

    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Idle),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 20.0,
            eta_ms: Some(2800),
            note: None,
        }),
    );
    b.task(
        300,
        EventKind::FileActivity(FileActivity {
            path: "schema/migration.sql".into(),
            op: FileOp::Created,
            from_path: None,
            lines_added: Some(35),
            lines_removed: None,
            bytes: None,
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 35.0,
            eta_ms: None,
            note: None,
        }),
    );

    // Block on a sibling task.
    let dep_since = b.future_ts(200);
    b.task(
        200,
        EventKind::Dependency(Dependency {
            waiting_on: vec!["task-migration-prereq".into()],
            reason: "needs schema migration from claude-1".into(),
            since: Some(dep_since),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Blocked,
            from: Some(WorkerState::Executing),
            note: Some("waiting on claude-1".into()),
        }),
    );

    // Wait, then unblock.
    b.task(
        800,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "claude-1 completed; resuming".into(),
            tag: Some("dep".into()),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Blocked),
            note: Some("unblocked".into()),
        }),
    );

    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 70.0,
            eta_ms: Some(800),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::FileActivity(FileActivity {
            path: "schema/migration.sql".into(),
            op: FileOp::Modified,
            from_path: None,
            lines_added: Some(8),
            lines_removed: Some(2),
            bytes: None,
        }),
    );
    b.task(
        300,
        EventKind::TestResult(TestResult {
            suite: "cargo test".into(),
            passed: 5,
            failed: 0,
            skipped: 0,
            duration_ms: 380,
            failures: None,
        }),
    );
    b.task(
        200,
        EventKind::Cost(Cost {
            input_tokens: 2980,
            output_tokens: 612,
            usd: 0.0118,
            cache_read_tokens: None,
            cache_write_tokens: None,
            model: Some("gpt-5".into()),
        }),
    );

    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Done,
            from: Some(WorkerState::Executing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Completion(Completion {
            summary: "Migration SQL applied; resumed after dependency clear; tests green.".into(),
            artifacts: Some(vec![Artifact {
                kind: ArtifactKind::File,
                artifact_ref: "schema/migration.sql".into(),
                label: Some("applied".into()),
            }]),
            duration_ms: Some(3400),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// gemini_happy: planning → executing → reviewing → done
// (mirror of claude_happy, different file paths + summary)
// ---------------------------------------------------------------------

pub fn gemini_happy(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );

    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Planning,
            from: Some(WorkerState::Idle),
            note: Some("drafting plan".into()),
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 12.0,
            eta_ms: Some(2800),
            note: Some("decomposed task".into()),
        }),
    );
    b.task(
        300,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "drafted plan: 3 steps".into(),
            tag: Some("plan".into()),
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Planning),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 30.0,
            eta_ms: Some(2400),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::FileActivity(FileActivity {
            path: "src/auth.rs".into(),
            op: FileOp::Modified,
            from_path: None,
            lines_added: Some(31),
            lines_removed: Some(7),
            bytes: None,
        }),
    );
    b.task(
        300,
        EventKind::Progress(Progress {
            percent: 55.0,
            eta_ms: Some(1500),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::FileActivity(FileActivity {
            path: "src/auth_test.rs".into(),
            op: FileOp::Created,
            from_path: None,
            lines_added: Some(18),
            lines_removed: None,
            bytes: None,
        }),
    );
    b.task(
        200,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "patch applied".into(),
            tag: None,
        }),
    );
    b.task(
        300,
        EventKind::Progress(Progress {
            percent: 80.0,
            eta_ms: Some(600),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Reviewing,
            from: Some(WorkerState::Executing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::TestResult(TestResult {
            suite: "cargo test".into(),
            passed: 14,
            failed: 0,
            skipped: 0,
            duration_ms: 980,
            failures: None,
        }),
    );
    b.task(
        200,
        EventKind::Cost(Cost {
            input_tokens: 3850,
            output_tokens: 720,
            usd: 0.0142,
            cache_read_tokens: Some(8800),
            cache_write_tokens: Some(0),
            model: Some("gemini-3-flash".into()),
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Done,
            from: Some(WorkerState::Reviewing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Completion(Completion {
            summary: "Refactored auth middleware; 1 test added; 14 tests pass.".into(),
            artifacts: Some(vec![
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/auth.rs".into(),
                    label: Some("modified".into()),
                },
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/auth_test.rs".into(),
                    label: Some("created".into()),
                },
            ]),
            duration_ms: Some(3000),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// gemini_blocked: executing → blocked → executing → done
// (mirror of codex_blocked, different copy + waiting target)
// ---------------------------------------------------------------------

pub fn gemini_blocked(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Idle),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Progress(Progress {
            percent: 25.0,
            eta_ms: Some(2200),
            note: None,
        }),
    );
    b.task(
        300,
        EventKind::FileActivity(FileActivity {
            path: "docs/api.md".into(),
            op: FileOp::Modified,
            from_path: None,
            lines_added: Some(12),
            lines_removed: Some(3),
            bytes: None,
        }),
    );
    let dep_since = b.future_ts(200);
    b.task(
        200,
        EventKind::Dependency(Dependency {
            waiting_on: vec!["task-spec-finalize".into()],
            reason: "blocked on spec finalization".into(),
            since: Some(dep_since),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Blocked,
            from: Some(WorkerState::Executing),
            note: Some("waiting on spec".into()),
        }),
    );
    b.task(
        700,
        EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "spec landed; resuming".into(),
            tag: Some("dep".into()),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Blocked),
            note: Some("unblocked".into()),
        }),
    );
    b.task(
        300,
        EventKind::Progress(Progress {
            percent: 75.0,
            eta_ms: Some(500),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Cost(Cost {
            input_tokens: 1840,
            output_tokens: 380,
            usd: 0.0072,
            cache_read_tokens: None,
            cache_write_tokens: None,
            model: Some("gemini-3-flash".into()),
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Done,
            from: Some(WorkerState::Executing),
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::Completion(Completion {
            summary: "Updated API docs after spec finalization.".into(),
            artifacts: None,
            duration_ms: Some(2700),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// gemini_failed: executing → failed (retryable: true)
// Replaces aider_failed for the supervisor's retry-policy tests.
// ---------------------------------------------------------------------

pub fn gemini_failed(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Idle),
            note: None,
        }),
    );
    b.task(
        300,
        EventKind::TestResult(TestResult {
            suite: "cargo test".into(),
            passed: 4,
            failed: 1,
            skipped: 0,
            duration_ms: 420,
            failures: Some(vec![TestFailure {
                name: "auth::tests::expired_token_rejected".into(),
                message: "expected Err(TokenExpired), got Ok(_)".into(),
                file: Some("src/auth.rs".into()),
                line: Some(184),
            }]),
        }),
    );
    b.task(
        200,
        EventKind::Failure(Failure {
            error: "auth test failure (expired_token_rejected)".into(),
            retryable: true,
            suggestion: Some("re-run after rebasing on main".into()),
            exit_code: Some(1),
            category: Some(FailureCategory::TaskLogic),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Failed,
            from: Some(WorkerState::Executing),
            note: Some("test failure".into()),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// gemini_terminal: executing → failed (retryable: false)
// Replaces aider_terminal — the worker emits Failure { retryable:
// false }, supervisor must NOT spawn a retry even when
// RetryPolicy::OnFailure is set.
// ---------------------------------------------------------------------

pub fn gemini_terminal(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );
    b.task(
        200,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Idle),
            note: None,
        }),
    );
    b.task(
        300,
        EventKind::Failure(Failure {
            error: "schema mismatch — non-retryable".into(),
            retryable: false,
            suggestion: Some("fix the schema before re-running".into()),
            exit_code: Some(2),
            category: Some(FailureCategory::TaskLogic),
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Failed,
            from: Some(WorkerState::Executing),
            note: Some("non-retryable".into()),
        }),
    );

    b.finish()
}

// ---------------------------------------------------------------------
// claude_quota_exhausted: executing → quota-exhausted (no completion)
// ---------------------------------------------------------------------

/// Emits a Claude-shaped trajectory that goes:
///   1. state_change idle
///   2. state_change executing
///   3. a `rate_limit_event` with status=exceeded so the
///      ClaudeAdapter's `take_quota_signal` returns Some.
///   4. NO completion — the supervisor's recovery engine pauses the
///      mission instead.
///
/// Used by the S5 e2e test
/// (`orchestrator/tests/recovery_quota_e2e.rs`) to verify the
/// pause-and-resume flow.
pub fn claude_quota_exhausted(opts: &EmitOpts) -> Vec<TimedEvent> {
    let mut b = Builder::new(opts);

    b.worker(
        0,
        EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    );
    b.task(
        100,
        EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Idle),
            note: None,
        }),
    );
    // The adapter consumes a raw JSON line with `type=rate_limit_event`,
    // not a canonical Event. The orchestrator only sees this script
    // by routing it through the supervisor's mock path which already
    // ingests raw lines from the adapter. For the recovery_quota_e2e
    // test we route the orchestrator's worker through the ClaudeAdapter
    // directly with a hand-rolled stdin; this function is the
    // canonical-event mirror so other consumers (e.g. the UI demo
    // mode) also see a representation of the trajectory.
    b.task(
        100,
        EventKind::Log(Log {
            level: LogLevel::Warn,
            stream: LogStream::Stdout,
            line: "rate_limit: exceeded (5-hour window)".into(),
            tag: Some("rate_limit".into()),
        }),
    );
    // No completion event. The supervisor's quota path takes over.
    b.finish()
}
