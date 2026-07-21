import type { Event, WorkerInfo } from "../bindings";

export type DemoRecordingId = "happy" | "blocked" | "quota";

export interface DemoRecording {
  id: DemoRecordingId;
  label: string;
  outcome: string;
  description: string;
  session: WorkerInfo;
  events: Event[];
}

const BASE_TIME = Date.parse("2026-07-21T16:00:00.000Z");

function recordedEvent<K extends Event["type"]>(
  workerId: string,
  taskId: string,
  seq: number,
  offsetMs: number,
  type: K,
  payload: Extract<Event, { type: K }>["payload"],
): Event {
  return {
    schema_version: "2.0",
    worker_id: workerId,
    task_id: taskId,
    seq,
    ts: new Date(BASE_TIME + offsetMs).toISOString(),
    type,
    payload,
  } as Event;
}

function session(id: DemoRecordingId, name: string): WorkerInfo {
  return {
    id: `demo-${id}`,
    name,
    vendor: "mock",
    cli_binary: "recorded-events",
    cli_version: null,
    cwd: "/read-only/demo",
    model: null,
    spawned_at: new Date(BASE_TIME).toISOString(),
    ended_at: new Date(BASE_TIME + 20_000).toISOString(),
  };
}

const happyEvents: Event[] = [
  recordedEvent("wkr-claude-01", "task-plan", 1, 0, "state_change", {
    state: "planning",
    note: "Decomposing the release into bounded work",
  }),
  recordedEvent("wkr-codex-02", "task-tests", 1, 350, "state_change", {
    state: "executing",
    note: "Adding regression coverage",
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 1, 700, "state_change", {
    state: "executing",
    note: "Refining the replay surface",
  }),
  recordedEvent("wkr-claude-01", "task-plan", 2, 1_050, "progress", {
    percent: 40,
    eta_ms: 7_000,
    note: "Scope and rollback checks passed",
  }),
  recordedEvent("wkr-codex-02", "task-tests", 2, 1_400, "file_activity", {
    path: "tests/replay.spec.ts",
    op: "created",
    lines_added: 86,
    lines_removed: 0,
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 2, 1_750, "file_activity", {
    path: "app/src/demo/WebDemoBanner.tsx",
    op: "modified",
    lines_added: 42,
    lines_removed: 8,
  }),
  recordedEvent("wkr-claude-01", "task-plan", 3, 2_100, "cost", {
    input_tokens: 1_280,
    output_tokens: 340,
    usd: 0.014,
    model: "claude-sonnet",
  }),
  recordedEvent("wkr-codex-02", "task-tests", 3, 2_450, "progress", {
    percent: 68,
    eta_ms: 4_000,
    note: "Running browser replay assertions",
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 3, 2_800, "progress", {
    percent: 72,
    eta_ms: 3_500,
    note: "Checking keyboard focus and reduced motion",
  }),
  recordedEvent("wkr-codex-02", "task-tests", 4, 3_150, "test_result", {
    suite: "web replay",
    passed: 18,
    failed: 0,
    skipped: 0,
    duration_ms: 1_842,
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 4, 3_500, "test_result", {
    suite: "accessibility smoke",
    passed: 9,
    failed: 0,
    skipped: 0,
    duration_ms: 1_106,
  }),
  recordedEvent("wkr-claude-01", "task-plan", 4, 3_850, "state_change", {
    state: "reviewing",
    from: "planning",
    note: "Auditing submissions against the authority envelope",
  }),
  recordedEvent("wkr-codex-02", "task-tests", 5, 4_200, "completion", {
    summary: "Regression suite extended; all checks pass.",
    duration_ms: 4_200,
  }),
  recordedEvent("wkr-codex-02", "task-tests", 6, 4_550, "state_change", {
    state: "done",
    from: "executing",
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 5, 4_900, "completion", {
    summary: "Replay controls polished and accessibility-checked.",
    duration_ms: 4_900,
  }),
  recordedEvent("wkr-antigravity-03", "task-ui", 6, 5_250, "state_change", {
    state: "done",
    from: "executing",
  }),
  recordedEvent("wkr-claude-01", "task-plan", 5, 5_600, "test_result", {
    suite: "integrated gate",
    passed: 27,
    failed: 0,
    skipped: 0,
    duration_ms: 2_208,
  }),
  recordedEvent("wkr-claude-01", "task-plan", 6, 5_950, "completion", {
    summary: "Audit accepted; snapshot-tagged merge is ready.",
    duration_ms: 5_950,
  }),
  recordedEvent("wkr-claude-01", "task-plan", 7, 6_300, "state_change", {
    state: "done",
    from: "reviewing",
  }),
];

const blockedEvents: Event[] = [
  recordedEvent("wkr-claude-01", "task-contract", 1, 0, "state_change", {
    state: "executing",
    note: "Updating the typed event contract",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 1, 450, "state_change", {
    state: "executing",
    note: "Wiring the downstream consumer",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 2, 900, "dependency", {
    waiting_on: ["task-contract"],
    reason: "The schema change must land before consumer generation",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 3, 1_350, "state_change", {
    state: "blocked",
    from: "executing",
    note: "Waiting for the event-schema contract",
  }),
  recordedEvent("wkr-claude-01", "task-contract", 2, 1_800, "file_activity", {
    path: "crates/event-schema/src/lib.rs",
    op: "modified",
    lines_added: 18,
    lines_removed: 4,
  }),
  recordedEvent("wkr-claude-01", "task-contract", 3, 2_250, "test_result", {
    suite: "schema compatibility",
    passed: 31,
    failed: 0,
    skipped: 0,
    duration_ms: 1_420,
  }),
  recordedEvent("wkr-claude-01", "task-contract", 4, 2_700, "completion", {
    summary: "Contract updated with backward-compatible fixtures.",
    duration_ms: 2_700,
  }),
  recordedEvent("wkr-claude-01", "task-contract", 5, 3_150, "state_change", {
    state: "done",
    from: "executing",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 4, 3_600, "state_change", {
    state: "executing",
    from: "blocked",
    note: "Dependency accepted; resuming from the saved session",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 5, 4_050, "test_result", {
    suite: "consumer regression",
    passed: 24,
    failed: 1,
    skipped: 0,
    duration_ms: 1_060,
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 6, 4_500, "failure", {
    error: "One compatibility assertion requires maintainer review.",
    retryable: false,
    suggestion: "Inspect the serialized v1 fixture before accepting the merge.",
    category: "task_logic",
  }),
  recordedEvent("wkr-codex-02", "task-consumer", 7, 4_950, "state_change", {
    state: "failed",
    from: "executing",
    note: "Escalated inside the Quality bound",
  }),
];

const quotaEvents: Event[] = [
  recordedEvent("wkr-antigravity-01", "task-index", 1, 0, "state_change", {
    state: "executing",
    note: "Indexing adapter fixtures",
  }),
  recordedEvent("wkr-antigravity-01", "task-index", 2, 500, "progress", {
    percent: 36,
    eta_ms: 9_000,
    note: "Processing the final fixture batch",
  }),
  recordedEvent("wkr-antigravity-01", "task-index", 3, 1_000, "cost", {
    input_tokens: 2_420,
    output_tokens: 510,
    usd: 0.021,
    model: "antigravity-agent",
  }),
  recordedEvent("wkr-antigravity-01", "task-index", 4, 1_500, "failure", {
    error: "Quota window reached; work is preserved for automatic resume.",
    retryable: true,
    suggestion: "Resume after the provider quota window reopens.",
    category: "rate_limit",
  }),
  recordedEvent("wkr-antigravity-01", "task-index", 5, 2_000, "state_change", {
    state: "blocked",
    from: "executing",
    note: "Paused until 16:30 UTC; no worktree changes were discarded",
  }),
];

export const DEMO_RECORDINGS: readonly DemoRecording[] = [
  {
    id: "happy",
    label: "Accepted",
    outcome: "27 checks passed · merge ready",
    description: "Three vendors execute in parallel, then pass an integrated audit.",
    session: session("happy", "Accepted mission"),
    events: happyEvents,
  },
  {
    id: "blocked",
    label: "Bound tripped",
    outcome: "Quality bound · review required",
    description: "A dependency resumes cleanly, then a failed assertion is escalated.",
    session: session("blocked", "Blocked mission"),
    events: blockedEvents,
  },
  {
    id: "quota",
    label: "Quota paused",
    outcome: "Work preserved · timed resume",
    description: "Provider quota pauses the worker without losing its worktree state.",
    session: session("quota", "Quota-paused mission"),
    events: quotaEvents,
  },
] as const;

export function findDemoRecording(id: DemoRecordingId): DemoRecording {
  const recording = DEMO_RECORDINGS.find((candidate) => candidate.id === id);
  if (!recording) throw new Error(`unknown demo recording: ${id}`);
  return recording;
}
