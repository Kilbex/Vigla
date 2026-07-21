import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the IPC layer so the runner doesn't reach real Tauri commands.
vi.mock("../bindings", () => ({
  commands: {
    startMockWorker: vi.fn(),
    startClaudeWorker: vi.fn(),
    startCodexWorker: vi.fn(),
  },
}));

import { commands } from "../bindings";
import { runPlaybook } from "../playbooks/runner";
import { TRIO_SWEEP, MIXED_DEMO } from "../playbooks/templates";
import { vendorOf, type PlaybookTemplate } from "../playbooks/types";
import { useOpsStore } from "../store";
import { initialReplayState } from "../replay/state";
import { emptyState } from "../store/ingest";

beforeEach(() => {
  vi.clearAllMocks();
  useOpsStore.setState({
    ...emptyState(),
    replay: initialReplayState,
    liveSnapshot: null,
  });
});

describe("playbook runner — happy path", () => {
  it("creates a squad with the template's name + color", async () => {
    (commands.startMockWorker as any).mockResolvedValue({
      status: "ok",
      data: "wid-fake",
    });
    const r = await runPlaybook(TRIO_SWEEP);
    const s = useOpsStore.getState();
    expect(s.squads[r.squadId]).toBeDefined();
    expect(s.squads[r.squadId].name).toBe(TRIO_SWEEP.squad.name);
    expect(s.squads[r.squadId].color).toBe(TRIO_SWEEP.squad.color);
  });

  it("calls startMockWorker once per mock member", async () => {
    (commands.startMockWorker as any).mockImplementation((script: string) =>
      Promise.resolve({ status: "ok", data: `w-${script}-${Math.random()}` }),
    );
    const r = await runPlaybook(TRIO_SWEEP);
    expect(commands.startMockWorker).toHaveBeenCalledTimes(
      TRIO_SWEEP.members.length,
    );
    expect(r.workerIds.length).toBe(TRIO_SWEEP.members.length);
    expect(r.errors).toEqual([]);
  });

  it("registers each spawned worker with the script-implied vendor", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      return Promise.resolve({ status: "ok", data: `w-${callCount}` });
    });
    await runPlaybook(MIXED_DEMO);
    const s = useOpsStore.getState();
    // MIXED_DEMO: lead=claude_happy → claude, blocker=codex_blocked → codex,
    // fault-injector=gemini_failed → gemini.
    const vendors = Object.values(s.workers).map((w) => w.vendor).sort();
    expect(vendors).toEqual(["claude", "codex", "gemini"]);
  });

  it("assigns every spawned worker to the new squad", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      return Promise.resolve({ status: "ok", data: `wid-${callCount}` });
    });
    const r = await runPlaybook(TRIO_SWEEP);
    const s = useOpsStore.getState();
    for (const wid of r.workerIds) {
      expect(s.workerSquad[wid]).toBe(r.squadId);
    }
    expect(s.squads[r.squadId].workerIds.sort()).toEqual(r.workerIds.sort());
  });

  it("uses the member role as the worker's currentTaskTitle", async () => {
    (commands.startMockWorker as any).mockResolvedValueOnce({
      status: "ok",
      data: "w-1",
    });
    (commands.startMockWorker as any).mockResolvedValueOnce({
      status: "ok",
      data: "w-2",
    });
    (commands.startMockWorker as any).mockResolvedValueOnce({
      status: "ok",
      data: "w-3",
    });
    await runPlaybook(TRIO_SWEEP);
    const titles = Object.values(useOpsStore.getState().workers)
      .map((w) => w.currentTaskTitle)
      .sort();
    expect(titles).toEqual(["implementer", "lead", "reviewer"]);
  });
});

describe("playbook runner — partial failures", () => {
  it("collects per-member spawn errors but does not abort the whole run", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      // Second call fails.
      if (callCount === 2) {
        return Promise.resolve({ status: "error", error: "supervisor died" });
      }
      return Promise.resolve({ status: "ok", data: `w-${callCount}` });
    });
    const r = await runPlaybook(TRIO_SWEEP);
    expect(r.workerIds.length).toBe(2);
    expect(r.errors.length).toBe(1);
    expect(r.errors[0].error).toBe("supervisor died");
  });

  it("squad still exists when all members fail to spawn", async () => {
    (commands.startMockWorker as any).mockResolvedValue({
      status: "error",
      error: "boom",
    });
    const r = await runPlaybook(TRIO_SWEEP);
    expect(r.workerIds).toEqual([]);
    expect(r.errors.length).toBe(TRIO_SWEEP.members.length);
    // Squad still present (operator can manually clean up).
    expect(useOpsStore.getState().squads[r.squadId]).toBeDefined();
    expect(useOpsStore.getState().squads[r.squadId].workerIds).toEqual([]);
  });

  it("a thrown exception in IPC is caught per-member", async () => {
    (commands.startMockWorker as any)
      .mockResolvedValueOnce({ status: "ok", data: "w-1" })
      .mockRejectedValueOnce(new Error("network unreachable"))
      .mockResolvedValueOnce({ status: "ok", data: "w-3" });
    const r = await runPlaybook(TRIO_SWEEP);
    expect(r.workerIds.length).toBe(2);
    expect(r.errors.length).toBe(1);
    expect(r.errors[0].error).toBe("network unreachable");
  });
});

describe("playbook runner — lead designation (Step 21)", () => {
  it("sets the squad's leadWorkerId to the first member with isLead=true", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      return Promise.resolve({ status: "ok", data: `w-${callCount}` });
    });
    const r = await runPlaybook(TRIO_SWEEP);
    const squad = useOpsStore.getState().squads[r.squadId];
    // TRIO_SWEEP has lead="lead" as the first member with isLead.
    expect(squad.leadWorkerId).toBeTruthy();
    expect(squad.leadWorkerId).toBe(r.workerIds[0]);
  });

  it("a playbook with no isLead member leaves leadWorkerId as null", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      return Promise.resolve({ status: "ok", data: `w-leadless-${callCount}` });
    });
    const tpl: PlaybookTemplate = {
      id: "t-no-lead",
      name: "No Lead",
      description: "test",
      squad: { name: "Leaderless", color: "plum" },
      members: [
        { role: "a", spawn: { kind: "mock", script: "claude_happy" } },
        { role: "b", spawn: { kind: "mock", script: "claude_happy" } },
      ],
    };
    const r = await runPlaybook(tpl);
    expect(useOpsStore.getState().squads[r.squadId].leadWorkerId).toBeNull();
  });

  it("first isLead wins when a playbook (mistakenly) marks two", async () => {
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      return Promise.resolve({ status: "ok", data: `w-multi-${callCount}` });
    });
    const tpl: PlaybookTemplate = {
      id: "t-multi-lead",
      name: "Multi-Lead",
      description: "test",
      squad: { name: "Two Leads", color: "sage" },
      members: [
        {
          role: "first",
          spawn: { kind: "mock", script: "claude_happy" },
          isLead: true,
        },
        {
          role: "second",
          spawn: { kind: "mock", script: "claude_happy" },
          isLead: true,
        },
      ],
    };
    const r = await runPlaybook(tpl);
    const squad = useOpsStore.getState().squads[r.squadId];
    expect(squad.leadWorkerId).toBe(r.workerIds[0]);
    expect(squad.leadWorkerId).not.toBe(r.workerIds[1]);
  });

  it("if the lead spawn fails, the next isLead member does NOT take over (Step 21 keeps it simple)", async () => {
    // First call (lead) fails; second call (would-be backup if any)
    // succeeds. Since only the FIRST isLead is considered and it
    // failed, no lead is set.
    let callCount = 0;
    (commands.startMockWorker as any).mockImplementation(() => {
      callCount += 1;
      if (callCount === 1) {
        return Promise.resolve({ status: "error", error: "lead spawn failed" });
      }
      return Promise.resolve({ status: "ok", data: `w-${callCount}` });
    });
    const r = await runPlaybook(TRIO_SWEEP);
    expect(useOpsStore.getState().squads[r.squadId].leadWorkerId).toBeNull();
    expect(r.errors[0].role).toBe("lead");
  });
});

describe("playbook runner — vendor dispatch", () => {
  it("routes a claude member to startClaudeWorker", async () => {
    const tpl: PlaybookTemplate = {
      id: "t-claude",
      name: "Claude Single",
      description: "test",
      squad: { name: "S", color: "indigo" },
      members: [
        {
          role: "doer",
          spawn: { kind: "claude", prompt: "do it", cwd: "/tmp", maxTurns: 4 },
        },
      ],
    };
    (commands.startClaudeWorker as any).mockResolvedValue({
      status: "ok",
      data: "w-claude",
    });
    await runPlaybook(tpl);
    expect(commands.startClaudeWorker).toHaveBeenCalledWith("do it", "/tmp", 4);
    expect(commands.startMockWorker).not.toHaveBeenCalled();
    expect(commands.startCodexWorker).not.toHaveBeenCalled();
  });

  it("routes a codex member to startCodexWorker", async () => {
    const tpl: PlaybookTemplate = {
      id: "t-codex",
      name: "Codex Single",
      description: "test",
      squad: { name: "S", color: "sage" },
      members: [
        {
          role: "doer",
          spawn: { kind: "codex", prompt: "do it", cwd: "/tmp" },
        },
      ],
    };
    (commands.startCodexWorker as any).mockResolvedValue({
      status: "ok",
      data: "w-codex",
    });
    await runPlaybook(tpl);
    expect(commands.startCodexWorker).toHaveBeenCalledWith("do it", "/tmp");
  });

  it("vendorOf maps mock scripts to the implied vendor", () => {
    expect(vendorOf({ kind: "mock", script: "claude_happy" })).toBe("claude");
    expect(vendorOf({ kind: "mock", script: "codex_blocked" })).toBe("codex");
    expect(vendorOf({ kind: "mock", script: "gemini_happy" })).toBe("gemini");
    expect(vendorOf({ kind: "mock", script: "gemini_failed" })).toBe("gemini");
    expect(vendorOf({ kind: "claude", prompt: "", cwd: "" })).toBe("claude");
    expect(vendorOf({ kind: "codex", prompt: "", cwd: "" })).toBe("codex");
  });

  it("vendorOf falls back to 'mock' for unknown future MockScript values (Step 22 forward-compat)", () => {
    // Disk-backed playbooks (Step 22) may ship a script value the
    // current build doesn't know about. Without the runtime
    // fallback, MOCK_VENDOR_MAP[unknown] would be undefined and
    // registerWorker would store undefined in WorkerSnapshot.vendor.
    expect(
      vendorOf({
        kind: "mock",
        script: "future_script_v2" as never,
      }),
    ).toBe("mock");
  });
});
