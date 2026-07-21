// S3 — assert silent-by-default at the ingest layer. The Rust
// integration test (orchestrator/tests/escalation_visibility.rs)
// covers the policy mapping; this test covers the *ingest
// reducer's* incorporation of that mapping.

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../bindings", () => ({
  commands: {
    missionEventVisibility: vi.fn(),
    surfaceInboxNotification: vi.fn(),
  },
}));

import { commands } from "../bindings";
import {
  _setBannerEmitter,
  _setInboxAppender,
  applyMissionEvent,
} from "../missions/ingest";
import { emptyMissionsState } from "../missions/types";
import type { MissionEvent } from "../bindings";
import type { InboxCard } from "../inbox/types";
import type { MissionsState } from "../missions/types";
import { _resetVisibilityCache } from "../inbox/visibility-client";

const MID = "demo-silent";

function ts(seq: number): string {
  return `2026-05-21T00:00:00.${String(seq).padStart(3, "0")}Z`;
}

function ev(seq: number, type: MissionEvent["type"], payload: unknown): MissionEvent {
  return { mission_id: MID, seq, ts: ts(seq), type, payload } as MissionEvent;
}

// Test-local appender that mirrors the production store wiring: when
// the async visibility lookup resolves with an Inbox verdict, the
// appender writes the card onto the active mission's inbox slice. The
// production wiring uses Zustand; the test holds the state mutably so
// the reducer-pure shape is preserved.
function makeAppender(
  ref: { current: MissionsState },
): (missionId: string, card: InboxCard) => void {
  return (missionId: string, card: InboxCard) => {
    const active = ref.current.active;
    if (!active || active.id !== missionId) return;
    // Upsert by id, sorted by seq ascending (matches applyInboxAction).
    const without = active.inbox.filter((c) => c.id !== card.id);
    const merged = [...without, card].sort((a, b) => a.seq - b.seq);
    ref.current = {
      ...ref.current,
      active: { ...active, inbox: merged },
    };
  };
}

beforeEach(() => {
  _resetVisibilityCache();
  vi.clearAllMocks();
  // Default — all events resolve to Internal. Specific tests
  // override per-call.
  (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockResolvedValue({
    kind: "internal",
  });
  _setInboxAppender(null);
  _setBannerEmitter(null);
});

describe("ingest produces no inbox entries during a happy-path mission", () => {
  it("emits zero ActionRequired cards", async () => {
    // The reducer is synchronous, so the visibility lookup runs
    // out-of-band — the ingest test verifies the slice content
    // *after* the lookup resolves. The mocked binding returns
    // immediate verdicts.
    const created = ev(0, "mission.created", {
      spec: {
        title: "T",
        objective: "O",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
    });

    const stream: MissionEvent[] = [
      created,
      ev(1, "mission.execution_started", null),
      ev(2, "supervisor.decomposition", {
        tasks: [{ index: 0, title: "Step 1" }],
      }),
      ev(3, "worker.spawned", {
        worker_id: "w-1",
        task_index: 0,
        task_title: "Step 1",
      }),
      ev(4, "worker.progress", { worker_id: "w-1", note: "looking" }),
      ev(5, "worker.result_submitted", {
        worker_id: "w-1",
        files: ["src/a.rs"],
        summary: "patched",
      }),
      ev(6, "supervisor.review_started", { worker_id: "w-1" }),
      ev(7, "supervisor.audit_completed", {
        tier: "smoke",
        overall: 0.85,
        payload_json: "{}",
      }),
      ev(8, "supervisor.integrated", {
        worker_id: "w-1",
        integration_sha: "sha",
        snapshot_tag: "snap",
      }),
    ];

    // Mock the verdict for each event to mirror the Rust mapping.
    (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockImplementation(
      async (kind: { type: string }) => {
        switch (kind.type) {
          case "mission.completed":
            return {
              kind: "inbox",
              inbox_kind: "completion",
              severity: "info",
            };
          default:
            // Power-user-only / internal — in either case, NOT
            // an inbox card.
            return { kind: "internal" };
        }
      },
    );

    let state = emptyMissionsState();
    const ref: { current: MissionsState } = { current: state };
    _setInboxAppender(makeAppender(ref));

    for (const event of stream) {
      state = applyMissionEvent(state, event);
      ref.current = state;
    }
    // Allow microtasks (the async visibility lookups) to flush.
    await Promise.resolve();
    await Promise.resolve();

    expect(ref.current.active).not.toBeNull();
    expect(ref.current.active!.inbox).toEqual([]);
  });

  it("emits one Info Completion card on mission.completed", async () => {
    (commands.missionEventVisibility as ReturnType<typeof vi.fn>).mockImplementation(
      async (kind: { type: string }) => {
        if (kind.type === "mission.completed") {
          return {
            kind: "inbox",
            inbox_kind: "completion",
            severity: "info",
          };
        }
        return { kind: "internal" };
      },
    );

    const created = ev(0, "mission.created", {
      spec: {
        title: "T",
        objective: "O",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
    });
    let state = emptyMissionsState();
    const ref: { current: MissionsState } = { current: state };
    _setInboxAppender(makeAppender(ref));

    state = applyMissionEvent(state, created);
    ref.current = state;
    state = applyMissionEvent(
      state,
      ev(1, "mission.completed", {
        summary: "1 task integrated",
        files_changed: 1,
      }),
    );
    ref.current = state;

    // The async visibility lookup may race the synchronous
    // reducer; the ingest under S3 enqueues a follow-up dispatch
    // when the verdict resolves. Allow microtasks to flush.
    await Promise.resolve();
    await Promise.resolve();

    expect(ref.current.active!.inbox.some((c) => c.kind === "completion")).toBe(
      true,
    );
  });
});
