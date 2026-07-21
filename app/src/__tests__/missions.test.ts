import { beforeEach, describe, expect, it } from "vitest";
import type { MissionEvent } from "../bindings";
import { applyMissionEvent } from "../missions/ingest";
import {
  selectActiveMission,
  selectAttentionCount,
  selectAttentionItems,
  selectAwaitingDisposition,
  selectCanStartMission,
  selectMissionLifecycle,
  selectMissionProgress,
  selectMissionTasks,
  selectMissionWorkers,
  selectSupervisorActivity,
} from "../missions/store";
import type { MissionsState } from "../missions/types";
import { emptyMissionsState } from "../missions/types";

const MID = "demo-7a3f";
const TS = (n: number) => `2026-05-12T00:00:00.${String(n).padStart(3, "0")}Z`;

function fold(events: MissionEvent[]): MissionsState {
  let state = emptyMissionsState();
  for (const e of events) {
    state = applyMissionEvent(state, e);
  }
  return state;
}

function created(seq = 0): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "mission.created",
    payload: {
      spec: {
        title: "Add logout endpoint",
        objective: "Add /api/logout, invalidate session, update docs.",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
    },
  };
}

function decomposition(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "supervisor.decomposition",
    payload: {
      tasks: [
        { index: 0, title: "Plan integration" },
        { index: 1, title: "Implement changes" },
        { index: 2, title: "Update documentation" },
      ],
    },
  };
}

function workerSpawned(seq: number, workerId: string, taskIndex: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "worker.spawned",
    payload: { worker_id: workerId, task_index: taskIndex, task_title: `Task ${taskIndex}` },
  };
}

function workerSubmitted(seq: number, workerId: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "worker.result_submitted",
    payload: { worker_id: workerId, files: [`MOCK_${workerId}.md`], summary: "work" },
  };
}

function reviewStarted(seq: number, workerId: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "supervisor.review_started",
    payload: { worker_id: workerId },
  };
}

function integrated(seq: number, workerId: string, n: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "supervisor.integrated",
    payload: {
      worker_id: workerId,
      integration_sha: `sha-${n}`.padEnd(40, "0"),
      snapshot_tag: `vigla/snap/${MID}/${n}`,
    },
  };
}

function completed(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "mission.completed",
    payload: { summary: "3 tasks integrated", files_changed: 3 },
  };
}

function attentionReady(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "mission.attention_ready",
  };
}

function workerProgress(seq: number, workerId: string, note: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "worker.progress",
    payload: { worker_id: workerId, note },
  };
}

function testResult(seq: number): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "supervisor.test_result",
    payload: { passed: true, summary: "mock tests pass" },
  };
}

function arbiterEscalated(
  seq: number,
  workerId: string,
  bound: "scope" | "reversibility" | "risk" | "quality" = "risk",
): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "arbiter.decided",
    payload: {
      worker_id: workerId,
      decision_json: JSON.stringify({
        kind: "escalate",
        bound,
        evidence: { summary: "Codex killed by signal", payload_json: null },
      }),
      audit_overall: 0,
      bound,
    },
  };
}

function n3MockMissionStream(): MissionEvent[] {
  const stream: MissionEvent[] = [
    created(0),
    {
      mission_id: MID,
      seq: 1,
      ts: TS(1),
      type: "mission.execution_started",
    },
    decomposition(2),
  ];
  let seq = 3;
  for (const [taskIndex, workerId] of ["mock-1", "mock-2", "mock-3"].entries()) {
    stream.push(workerSpawned(seq++, workerId, taskIndex));
    stream.push(workerProgress(seq++, workerId, `Working on task ${taskIndex + 1}`));
    stream.push(workerSubmitted(seq++, workerId));
    stream.push(reviewStarted(seq++, workerId));
    stream.push(integrated(seq++, workerId, taskIndex));
    stream.push(testResult(seq++));
  }
  stream.push(completed(seq));
  return stream;
}

function mergeResolved(
  seq: number,
  resolution: "merged" | "discarded",
): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "mission.merge_resolved",
    payload: { resolution: { type: resolution } },
  };
}

function aborted(seq: number, reason: string): MissionEvent {
  return {
    mission_id: MID,
    seq,
    ts: TS(seq),
    type: "mission.aborted",
    payload: { reason },
  };
}

describe("mission ingest", () => {
  let state: MissionsState;
  beforeEach(() => {
    state = emptyMissionsState();
  });

  it("created event populates the active mission", () => {
    state = applyMissionEvent(state, created());
    const m = selectActiveMission(state);
    expect(m).not.toBeNull();
    expect(m?.id).toBe(MID);
    expect(m?.spec.title).toBe("Add logout endpoint");
    expect(m?.lifecycle).toBe("created");
    expect(selectMissionTasks(state)).toEqual([]);
  });

  it("decomposition populates tasks as pending", () => {
    state = fold([created(0), decomposition(1)]);
    const tasks = selectMissionTasks(state);
    expect(tasks).toHaveLength(3);
    expect(tasks.every((t) => t.status === "pending")).toBe(true);
  });

  it("worker.spawned creates worker and moves task to in_progress", () => {
    state = fold([created(0), decomposition(1), workerSpawned(2, "mock-1", 0)]);
    const tasks = selectMissionTasks(state);
    expect(tasks[0].status).toBe("in_progress");
    expect(tasks[0].assignedWorkerId).toBe("mock-1");
    const workers = selectMissionWorkers(state);
    expect(workers["mock-1"].status).toBe("spawned");
  });

  it("supervisor.integrated marks task integrated and bumps progress", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
    ]);
    const tasks = selectMissionTasks(state);
    expect(tasks[0].status).toBe("integrated");
    expect(tasks[0].integrationSha?.startsWith("sha-0")).toBe(true);
    expect(tasks[0].snapshotTag).toBe(`vigla/snap/${MID}/0`);
    // 1 of 3 integrated → 33%.
    expect(selectMissionProgress(state)).toBe(33);
  });

  it("review_started flips lifecycle to reviewing", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
    ]);
    expect(selectMissionLifecycle(state)).toBe("reviewing");
  });

  it("mission.completed surfaces an attention item and awaits disposition", () => {
    state = fold([created(0), decomposition(1), completed(2)]);
    expect(selectMissionLifecycle(state)).toBe("complete_pending_merge");
    expect(selectAwaitingDisposition(state)).toBe(true);
    expect(selectAttentionCount(state)).toBe(1);
    expect(selectAttentionItems(state)[0].kind).toBe("mission_complete");
    expect(selectMissionProgress(state)).toBe(100);
  });

  it("mission.merge_resolved (merged) clears the complete attention and goes terminal", () => {
    state = fold([created(0), decomposition(1), completed(2), mergeResolved(3, "merged")]);
    expect(selectMissionLifecycle(state)).toBe("merged");
    expect(selectAttentionCount(state)).toBe(0);
    expect(selectCanStartMission(state)).toBe(true);
    expect(selectActiveMission(state)?.resolution?.type).toBe("merged");
  });

  it("mission.merge_resolved (discarded) also goes terminal", () => {
    state = fold([created(0), decomposition(1), completed(2), mergeResolved(3, "discarded")]);
    expect(selectMissionLifecycle(state)).toBe("discarded");
    expect(selectCanStartMission(state)).toBe(true);
  });

  it("mission.aborted surfaces an aborted attention and is terminal", () => {
    state = fold([created(0), aborted(1, "user abort")]);
    expect(selectMissionLifecycle(state)).toBe("aborted");
    expect(selectAttentionCount(state)).toBe(1);
    expect(selectAttentionItems(state)[0].kind).toBe("mission_aborted");
    expect(selectCanStartMission(state)).toBe(true);
  });

  it("events for an unknown mission_id are ignored", () => {
    state = applyMissionEvent(emptyMissionsState(), {
      mission_id: "other-mid",
      seq: 0,
      ts: TS(0),
      type: "worker.spawned",
      payload: { worker_id: "mock-1", task_index: 0, task_title: "x" },
    });
    expect(selectActiveMission(state)).toBeNull();
  });

  it("a new mission.created replaces the prior mission", () => {
    state = fold([
      created(0),
      decomposition(1),
      completed(2),
      mergeResolved(3, "merged"),
    ]);
    const replacement: MissionEvent = {
      mission_id: "fresh-1234",
      seq: 0,
      ts: TS(10),
      type: "mission.created",
      payload: {
        spec: {
          title: "Next mission",
          objective: "Do something else",
          target_ref: "main",
          tests: null,
          supervisor_model: null,
          worker_model: null,
          worker_count: null,
          confirm_plan: null,
        },
      },
    };
    state = applyMissionEvent(state, replacement);
    expect(selectActiveMission(state)?.id).toBe("fresh-1234");
    expect(selectMissionLifecycle(state)).toBe("created");
    expect(selectAttentionCount(state)).toBe(0);
  });

  it("can start mission when no mission is active", () => {
    expect(selectCanStartMission(emptyMissionsState())).toBe(true);
  });

  it("cannot start mission while one is executing", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
    ]);
    expect(selectCanStartMission(state)).toBe(false);
  });

  it("worker progress updates the status line", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      {
        mission_id: MID,
        seq: 3,
        ts: TS(3),
        type: "worker.progress",
        payload: { worker_id: "mock-1", note: "Wiring middleware" },
      },
    ]);
    expect(selectActiveMission(state)?.statusLine).toBe("Wiring middleware");
    expect(selectMissionWorkers(state)["mock-1"].latestProgress).toBe(
      "Wiring middleware",
    );
    expect(selectMissionWorkers(state)["mock-1"].status).toBe("working");
  });

  it("full success path ends in complete_pending_merge with 100% and 1 attention", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
      workerSpawned(6, "mock-2", 1),
      workerSubmitted(7, "mock-2"),
      reviewStarted(8, "mock-2"),
      integrated(9, "mock-2", 1),
      workerSpawned(10, "mock-3", 2),
      workerSubmitted(11, "mock-3"),
      reviewStarted(12, "mock-3"),
      integrated(13, "mock-3", 2),
      completed(14),
    ]);
    expect(selectMissionLifecycle(state)).toBe("complete_pending_merge");
    expect(selectMissionProgress(state)).toBe(100);
    expect(selectAttentionCount(state)).toBe(1);
    expect(selectMissionTasks(state).every((t) => t.status === "integrated")).toBe(true);
  });

  it("arbiter escalation moves mission to attention and late integrations do not clear it", () => {
    state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSpawned(3, "mock-2", 1),
      arbiterEscalated(4, "mock-1", "risk"),
      attentionReady(5),
      workerSubmitted(6, "mock-2"),
      reviewStarted(7, "mock-2"),
      integrated(8, "mock-2", 1),
    ]);

    expect(selectMissionLifecycle(state)).toBe("attention");
    expect(selectSupervisorActivity(state)).toBe("supervisor: paused — see Attention");
    expect(selectAttentionItems(state)).toEqual([
      expect.objectContaining({
        kind: "arbiter_escalation",
        severity: "hard",
        summary: 'Escalation (risk) on "Task 0"',
      }),
    ]);
    expect(selectActiveMission(state)?.statusLine).toBe(
      'Escalation (risk) on "Task 0"',
    );
    expect(selectMissionTasks(state)[0].status).toBe("failed");
    expect(selectMissionTasks(state)[1].status).toBe("integrated");
  });

  it("N=3 mock mission ingest stays within one 60fps frame", () => {
    const stream = n3MockMissionStream();
    const t0 = performance.now();
    const next = fold(stream);
    const elapsed = performance.now() - t0;

    expect(elapsed).toBeLessThan(16);
    expect(selectMissionLifecycle(next)).toBe("complete_pending_merge");
    expect(selectMissionProgress(next)).toBe(100);
    expect(selectMissionTasks(next)).toHaveLength(3);
    expect(Object.keys(selectMissionWorkers(next))).toHaveLength(3);
  });

  // ──────────────────────────────────────────────────────────────
  // QC-2: plan-preview event flow.
  // ──────────────────────────────────────────────────────────────

  function planProposed(seq: number, generation: number): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "plan.proposed",
      payload: {
        tasks: [
          { index: 0, title: "First proposed task" },
          { index: 1, title: "Second proposed task" },
        ],
        generation,
      },
    };
  }

  function planConfirmed(seq: number, generation: number): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "plan.confirmed",
      payload: { generation },
    };
  }

  function planRegenerationRequested(
    seq: number,
    hint: string | null,
    prior_generation: number,
  ): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "plan.regeneration_requested",
      payload: { hint, prior_generation },
    };
  }

  it("plan.proposed pauses lifecycle at pending_plan_approval", () => {
    const state = fold([created(0), planProposed(1, 0)]);
    expect(selectMissionLifecycle(state)).toBe("pending_plan_approval");
    expect(selectMissionTasks(state).map((t) => t.title)).toEqual([
      "First proposed task",
      "Second proposed task",
    ]);
    expect(state.active?.planGeneration).toBe(0);
  });

  it("plan.confirmed unblocks lifecycle to executing", () => {
    const state = fold([created(0), planProposed(1, 0), planConfirmed(2, 0)]);
    expect(selectMissionLifecycle(state)).toBe("executing");
  });

  it("plan.regeneration_requested keeps lifecycle paused with a regenerating status line", () => {
    const state = fold([
      created(0),
      planProposed(1, 0),
      planRegenerationRequested(2, "Make the tasks smaller", 0),
    ]);
    expect(selectMissionLifecycle(state)).toBe("pending_plan_approval");
    expect(state.active?.statusLine).toMatch(/regenerating/i);
  });

  it("regenerate → next plan.proposed bumps planGeneration and replaces tasks", () => {
    const state = fold([
      created(0),
      planProposed(1, 0),
      planRegenerationRequested(2, "more granular", 0),
      planProposed(3, 1),
    ]);
    expect(state.active?.planGeneration).toBe(1);
    expect(selectMissionLifecycle(state)).toBe("pending_plan_approval");
  });

  it("full QC-2 path: proposed → regenerate → re-proposed → confirmed → executing", () => {
    const state = fold([
      created(0),
      planProposed(1, 0),
      planRegenerationRequested(2, null, 0),
      planProposed(3, 1),
      planConfirmed(4, 1),
    ]);
    expect(selectMissionLifecycle(state)).toBe("executing");
    expect(selectCanStartMission(state)).toBe(false);
  });

  it("planGeneration defaults to 0 for missions that never pause for approval", () => {
    const state = fold([created(0)]);
    expect(state.active?.planGeneration).toBe(0);
  });

  // ---------------------------------------------------------------
  // Phase 1 — decisions.md entries 5 (Cost authority) & 6
  // (Single supervisor per mission).
  // ---------------------------------------------------------------

  function subSupervisorRefused(seq: number): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "boundary.sub_supervisor_refused",
      payload: {
        requested_by_supervisor_id: "sup-root",
        requested_worker_id: "mock-sub-1",
      },
    };
  }

  function sideEffectLogged(
    seq: number,
    declared: boolean,
  ): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "boundary.side_effect_logged",
      payload: {
        worker_id: "mock-1",
        kind: "package_install",
        summary: "package install observed: python -m pip install",
        declared,
      },
    };
  }

  it("boundary.side_effect_logged pushes a soft Attention item without blocking the mission", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      sideEffectLogged(3, true),
    ]);
    const items = selectAttentionItems(state);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("side_effect_logged");
    expect(items[0].severity).toBe("soft");
    expect(items[0].summary).toMatch(/package_install/);
    expect(items[0].summary).toMatch(/declared/);
    expect(selectMissionLifecycle(state)).toBe("executing");
  });

  it("boundary.sub_supervisor_refused pushes a soft Attention item without changing lifecycle", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      subSupervisorRefused(3),
    ]);
    const items = selectAttentionItems(state);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("sub_supervisor_refused");
    expect(items[0].severity).toBe("soft");
    expect(items[0].summary).toMatch(/single-supervisor-per-mission/i);
    expect(selectMissionLifecycle(state)).toBe("executing");
  });

  // ---------------------------------------------------------------
  // Supervisor strip + historical Extend replay compatibility.
  // ---------------------------------------------------------------

  function executionStarted(seq: number): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "mission.execution_started",
    };
  }

  function missionExtended(
    seq: number,
    directive: string | null,
  ): MissionEvent {
    return {
      mission_id: MID,
      seq,
      ts: TS(seq),
      type: "mission.extended",
      payload: { directive },
    };
  }

  it("supervisorStrip_spawned_event_produces_starting_up", () => {
    // No `supervisor.spawned` exists on the wire; the closest
    // existing event for "supervisor is spinning up" is
    // `mission.execution_started`. The strip surfaces that as
    // "starting up".
    const state = fold([created(0), executionStarted(1)]);
    expect(selectSupervisorActivity(state)).toBe("supervisor: starting up");
  });

  it("supervisorStrip_decomposition_event_produces_planning_tasks", () => {
    const state = fold([created(0), decomposition(1)]);
    expect(selectSupervisorActivity(state)).toBe("supervisor: planning tasks");
  });

  it("supervisorStrip_review_started_includes_worker_id", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-claude-1", 0),
      workerSubmitted(3, "mock-claude-1"),
      reviewStarted(4, "mock-claude-1"),
    ]);
    expect(selectSupervisorActivity(state)).toBe(
      "supervisor: reviewing mock-claude-1's work",
    );
  });

  it("supervisorStrip_decision_accept_includes_worker_id", () => {
    // The closest existing wire event for an accept decision is
    // `supervisor.integrated`; the strip surfaces that.
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-codex-2", 0),
      workerSubmitted(3, "mock-codex-2"),
      reviewStarted(4, "mock-codex-2"),
      integrated(5, "mock-codex-2", 0),
    ]);
    expect(selectSupervisorActivity(state)).toBe(
      "supervisor: integrated mock-codex-2",
    );
  });

  it("supervisorStrip_complete_pending_merge_says_awaiting_decision", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
      completed(6),
    ]);
    expect(selectSupervisorActivity(state)).toBe(
      "supervisor: awaiting your decision",
    );
  });

  it("supervisorStrip_no_supervisor_event_returns_null", () => {
    // mission.created alone (no execution_started yet) leaves the
    // strip hidden — the team-view component renders nothing.
    const state = fold([created(0)]);
    expect(selectSupervisorActivity(state)).toBeNull();
  });

  it("supervisorStrip_truncates_long_historical_extension_directive", () => {
    const longDirective =
      "Refine the auth flow to add MFA, rate-limiting, and an audit log of every login attempt across all tenants";
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
      completed(6),
      missionExtended(7, longDirective),
    ]);
    const activity = selectSupervisorActivity(state);
    expect(activity).toMatch(/^supervisor: legacy extension request — /);
    // Total body length is bounded by the derive's 60-char cap.
    expect(activity!.length).toBeLessThan(longDirective.length + 32);
    expect(activity!.endsWith("…")).toBe(true);
  });

  it("mission_extended_event_updates_lastExtensionDirective", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
      completed(6),
      missionExtended(7, "widen retry policy"),
    ]);
    expect(state.active?.lastExtensionDirective).toBe("widen retry policy");
    expect(state.active?.lastExtensionAt).toBe(TS(7));
    expect(selectSupervisorActivity(state)).toBe(
      "supervisor: legacy extension request — widen retry policy",
    );
  });

  it("mission_extended_with_null_directive_records_null", () => {
    const state = fold([
      created(0),
      decomposition(1),
      workerSpawned(2, "mock-1", 0),
      workerSubmitted(3, "mock-1"),
      reviewStarted(4, "mock-1"),
      integrated(5, "mock-1", 0),
      completed(6),
      missionExtended(7, null),
    ]);
    expect(state.active?.lastExtensionDirective).toBeNull();
    expect(selectSupervisorActivity(state)).toBe(
      "supervisor: legacy extension request",
    );
  });
});
