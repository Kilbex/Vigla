import { commands } from "../bindings";
import { useOpsStore } from "../store";
import { vendorOf, type PlaybookMember, type PlaybookTemplate } from "./types";

export interface RunResult {
  squadId: string;
  /// The workerIds successfully spawned. Members that failed to spawn
  /// produce `errors` entries instead and are NOT added to the squad.
  workerIds: string[];
  errors: Array<{ role: string; error: string }>;
}

interface SpawnOutcome {
  member: PlaybookMember;
  workerId: string | null;
  error: string | null;
}

async function spawnMember(member: PlaybookMember): Promise<SpawnOutcome> {
  try {
    if (member.spawn.kind === "mock") {
      const r = await commands.startMockWorker(member.spawn.script, 1.0);
      if (r.status === "ok") return { member, workerId: r.data, error: null };
      return { member, workerId: null, error: r.error };
    }
    if (member.spawn.kind === "claude") {
      const r = await commands.startClaudeWorker(
        member.spawn.prompt,
        member.spawn.cwd,
        member.spawn.maxTurns ?? null,
      );
      if (r.status === "ok") return { member, workerId: r.data, error: null };
      return { member, workerId: null, error: r.error };
    }
    if (member.spawn.kind === "codex") {
      const r = await commands.startCodexWorker(
        member.spawn.prompt,
        member.spawn.cwd,
      );
      if (r.status === "ok") return { member, workerId: r.data, error: null };
      return { member, workerId: null, error: r.error };
    }
    // gemini
    const r = await commands.startGeminiWorker(
      member.spawn.prompt,
      member.spawn.cwd,
    );
    if (r.status === "ok") return { member, workerId: r.data, error: null };
    return { member, workerId: null, error: r.error };
  } catch (e) {
    return {
      member,
      workerId: null,
      error: e instanceof Error ? e.message : String(e),
    };
  }
}

/// Execute a playbook: create the squad, spawn each member in
/// parallel, register identities, and assign successful spawns to
/// the squad. Partial failures collect into `errors`; the squad
/// stays even if some members fail (operator can manually clean up).
export async function runPlaybook(
  template: PlaybookTemplate,
): Promise<RunResult> {
  const squadId = useOpsStore
    .getState()
    .createSquad(template.squad.name, template.squad.color);

  const outcomes = await Promise.all(template.members.map(spawnMember));

  const workerIds: string[] = [];
  const errors: RunResult["errors"] = [];
  let leadWorkerId: string | null = null;
  for (const o of outcomes) {
    if (o.workerId === null) {
      errors.push({ role: o.member.role, error: o.error ?? "unknown error" });
      continue;
    }
    workerIds.push(o.workerId);
    // Pre-populate vendor + role label so the station tile reads
    // correctly before the first event arrives. registerWorker is
    // patch-or-create per the Step 17 polish contract.
    useOpsStore
      .getState()
      .registerWorker(o.workerId, vendorOf(o.member.spawn), o.member.role);
    useOpsStore.getState().assignWorkerToSquad(o.workerId, squadId);
    // Step 21 — first member with isLead becomes the squad lead.
    // Subsequent isLead members are ignored (one lead per squad).
    if (o.member.isLead && leadWorkerId === null) {
      leadWorkerId = o.workerId;
    }
  }

  if (leadWorkerId !== null) {
    useOpsStore.getState().setSquadLead(squadId, leadWorkerId);
  }

  return { squadId, workerIds, errors };
}
