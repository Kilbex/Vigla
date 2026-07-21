import { describe, it, expect, beforeEach } from "vitest";
import { useOpsStore } from "../store";
import { initialReplayState } from "../replay/state";
import { emptyState } from "../store/ingest";
import type { Event, EventKind } from "../bindings";

function evt(
  workerId: string,
  seq: number,
  ts: string,
  taskId: string | null,
  kind: EventKind,
): Event {
  return {
    schema_version: "1.0",
    worker_id: workerId,
    task_id: taskId,
    seq,
    ts,
    ...kind,
  } as Event;
}

function resetStore() {
  useOpsStore.setState({
    ...emptyState(),
    replay: initialReplayState,
    liveSnapshot: null,
  });
}

beforeEach(() => {
  resetStore();
});

describe("squads — CRUD", () => {
  it("createSquad creates an empty squad with the given name + color and appends to order", () => {
    const id = useOpsStore.getState().createSquad("Frontend Squad", "indigo");
    const s = useOpsStore.getState();
    const sq = s.squads[id];
    expect(sq).toBeDefined();
    expect(sq.name).toBe("Frontend Squad");
    expect(sq.color).toBe("indigo");
    expect(sq.workerIds).toEqual([]);
    expect(s.squadOrder).toEqual([id]);
  });

  it("createSquad returns a unique ID per call", () => {
    const a = useOpsStore.getState().createSquad("A", "indigo");
    const b = useOpsStore.getState().createSquad("B", "sage");
    expect(a).not.toBe(b);
    expect(useOpsStore.getState().squadOrder).toEqual([a, b]);
  });

  it("renameSquad updates the name", () => {
    const id = useOpsStore.getState().createSquad("draft", "sand");
    useOpsStore.getState().renameSquad(id, "Backend Squad");
    expect(useOpsStore.getState().squads[id].name).toBe("Backend Squad");
  });

  it("setSquadColor updates the color", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    useOpsStore.getState().setSquadColor(id, "coral");
    expect(useOpsStore.getState().squads[id].color).toBe("coral");
  });

  it("deleteSquad removes the squad, prunes squadOrder, and clears workerSquad pointers", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    // Seed a worker so we can assign it.
    useOpsStore.getState().ingest(
      evt("w1", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    expect(useOpsStore.getState().workerSquad["w1"]).toBe(id);

    useOpsStore.getState().deleteSquad(id);
    const s = useOpsStore.getState();
    expect(s.squads[id]).toBeUndefined();
    expect(s.squadOrder).toEqual([]);
    expect(s.workerSquad["w1"]).toBeUndefined();
  });

  it("renameSquad / setSquadColor / deleteSquad on an unknown id are no-ops", () => {
    const before = useOpsStore.getState();
    useOpsStore.getState().renameSquad("bogus", "x");
    useOpsStore.getState().setSquadColor("bogus", "indigo");
    useOpsStore.getState().deleteSquad("bogus");
    const after = useOpsStore.getState();
    expect(after.squads).toEqual(before.squads);
    expect(after.squadOrder).toEqual(before.squadOrder);
  });
});

describe("squads — worker assignment", () => {
  it("assignWorkerToSquad sets workerSquad and adds to Squad.workerIds", () => {
    const id = useOpsStore.getState().createSquad("Frontend", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    const s = useOpsStore.getState();
    expect(s.workerSquad["w1"]).toBe(id);
    expect(s.squads[id].workerIds).toEqual(["w1"]);
  });

  it("reassigning a worker removes it from its prior squad and adds to the new one", () => {
    const a = useOpsStore.getState().createSquad("A", "indigo");
    const b = useOpsStore.getState().createSquad("B", "coral");
    useOpsStore.getState().assignWorkerToSquad("w1", a);
    useOpsStore.getState().assignWorkerToSquad("w1", b);
    const s = useOpsStore.getState();
    expect(s.workerSquad["w1"]).toBe(b);
    expect(s.squads[a].workerIds).toEqual([]);
    expect(s.squads[b].workerIds).toEqual(["w1"]);
  });

  it("assignWorkerToSquad(null) clears the assignment from both sides", () => {
    const id = useOpsStore.getState().createSquad("A", "sage");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    useOpsStore.getState().assignWorkerToSquad("w1", null);
    const s = useOpsStore.getState();
    expect(s.workerSquad["w1"]).toBeUndefined();
    expect(s.squads[id].workerIds).toEqual([]);
  });

  it("assignWorkerToSquad to an unknown squad id is a no-op", () => {
    useOpsStore.getState().assignWorkerToSquad("w1", "bogus");
    expect(useOpsStore.getState().workerSquad["w1"]).toBeUndefined();
  });

  it("assigning the same worker to the same squad twice is idempotent (no duplicate in workerIds)", () => {
    const id = useOpsStore.getState().createSquad("A", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    expect(useOpsStore.getState().squads[id].workerIds).toEqual(["w1"]);
  });
});

describe("squads — replay-aware actions", () => {
  it("createSquad during replay writes to liveSnapshot, not the visible projection", () => {
    useOpsStore.getState().enterReplay([]);
    const id = useOpsStore.getState().createSquad("Replay Squad", "plum");
    const s = useOpsStore.getState();
    // Visible state is the replay projection — should NOT have it.
    expect(s.squads[id]).toBeUndefined();
    expect(s.squadOrder).not.toContain(id);
    // liveSnapshot has it.
    expect(s.liveSnapshot!.squads[id]?.name).toBe("Replay Squad");
    expect(s.liveSnapshot!.squadOrder).toContain(id);
  });

  it("squads created during replay survive exitReplay", () => {
    useOpsStore.getState().enterReplay([]);
    const id = useOpsStore.getState().createSquad("Persistent", "sand");
    useOpsStore.getState().exitReplay();
    const s = useOpsStore.getState();
    expect(s.squads[id]?.name).toBe("Persistent");
    expect(s.squadOrder).toContain(id);
  });

  it("assignWorkerToSquad during replay writes to liveSnapshot", () => {
    // Seed live worker first.
    useOpsStore.getState().ingest(
      evt("w-live", 0, "2026-05-09T00:00:00.000Z", null, {
        type: "state_change",
        payload: { state: "idle" },
      }),
    );
    useOpsStore.getState().enterReplay([]);
    // liveSnapshot now has w-live; visible is the (empty) replay projection.
    const id = useOpsStore.getState().createSquad("During", "terracotta");
    useOpsStore.getState().assignWorkerToSquad("w-live", id);
    const s = useOpsStore.getState();
    expect(s.liveSnapshot!.workerSquad["w-live"]).toBe(id);
    expect(s.liveSnapshot!.squads[id].workerIds).toEqual(["w-live"]);
    // Visible should NOT have the assignment.
    expect(s.workerSquad["w-live"]).toBeUndefined();
  });
});

describe("squads — lead designation (Step 21)", () => {
  it("createSquad initializes leadWorkerId to null", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    expect(useOpsStore.getState().squads[id].leadWorkerId).toBeNull();
  });

  it("setSquadLead requires the worker to be a member of the squad", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    // workerId not yet a member — should be a no-op.
    useOpsStore.getState().setSquadLead(id, "wid-not-a-member");
    expect(useOpsStore.getState().squads[id].leadWorkerId).toBeNull();
  });

  it("setSquadLead designates a member as lead and clears with null", () => {
    const id = useOpsStore.getState().createSquad("X", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    useOpsStore.getState().setSquadLead(id, "w1");
    expect(useOpsStore.getState().squads[id].leadWorkerId).toBe("w1");
    useOpsStore.getState().setSquadLead(id, null);
    expect(useOpsStore.getState().squads[id].leadWorkerId).toBeNull();
  });

  it("setSquadLead on an unknown squad id is a no-op", () => {
    const before = useOpsStore.getState();
    useOpsStore.getState().setSquadLead("bogus", "w1");
    expect(useOpsStore.getState().squads).toEqual(before.squads);
  });

  it("reassigning a lead worker out of its squad clears the lead pointer", () => {
    const a = useOpsStore.getState().createSquad("A", "indigo");
    const b = useOpsStore.getState().createSquad("B", "coral");
    useOpsStore.getState().assignWorkerToSquad("w1", a);
    useOpsStore.getState().setSquadLead(a, "w1");
    expect(useOpsStore.getState().squads[a].leadWorkerId).toBe("w1");

    // Move worker to squad B — leadership doesn't follow.
    useOpsStore.getState().assignWorkerToSquad("w1", b);
    expect(useOpsStore.getState().squads[a].leadWorkerId).toBeNull();
    expect(useOpsStore.getState().squads[b].leadWorkerId).toBeNull();
  });

  it("unassigning a lead worker (squadId=null) clears the lead pointer", () => {
    const id = useOpsStore.getState().createSquad("A", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    useOpsStore.getState().setSquadLead(id, "w1");
    useOpsStore.getState().assignWorkerToSquad("w1", null);
    expect(useOpsStore.getState().squads[id].leadWorkerId).toBeNull();
  });
});

describe("squads — immutability", () => {
  it("createSquad does not mutate prev squads dict", () => {
    const before = useOpsStore.getState().squads;
    useOpsStore.getState().createSquad("A", "indigo");
    expect(useOpsStore.getState().squads).not.toBe(before);
    // The original empty dict is still empty (no retroactive mutation).
    expect(Object.keys(before)).toEqual([]);
  });

  it("assignWorkerToSquad does not mutate prev Squad.workerIds array", () => {
    const id = useOpsStore.getState().createSquad("A", "indigo");
    const beforeIds = useOpsStore.getState().squads[id].workerIds;
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    expect(beforeIds).toEqual([]); // not retroactively mutated
    expect(useOpsStore.getState().squads[id].workerIds).toEqual(["w1"]);
  });

  it("reset clears squads, squadOrder, and workerSquad", () => {
    const id = useOpsStore.getState().createSquad("A", "indigo");
    useOpsStore.getState().assignWorkerToSquad("w1", id);
    useOpsStore.getState().reset();
    const s = useOpsStore.getState();
    expect(s.squads).toEqual({});
    expect(s.squadOrder).toEqual([]);
    expect(s.workerSquad).toEqual({});
  });
});
