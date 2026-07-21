import { commands } from "../bindings";
import { BUILTIN_PLAYBOOKS } from "./templates";
import { SQUAD_COLORS, type SquadColor } from "../store/types";
import type { PlaybookMember, PlaybookMemberSpawn, PlaybookTemplate } from "./types";

/// A loaded playbook entry, tagged with where it came from. Built-ins
/// are read-only (the user can't delete or rename them); saved
/// playbooks live on disk under `<app_local_data_dir>/playbooks/<id>.json`.
export type PlaybookSource = "builtin" | "saved";

export interface PlaybookEntry {
  template: PlaybookTemplate;
  source: PlaybookSource;
}

/// Async, merged list. Built-ins first, then saved playbooks
/// alphabetically by id (the Rust store sorts on read). Saved
/// playbooks that fail validation are dropped with a console warn so
/// the rest of the panel keeps working.
export async function loadAllPlaybooks(): Promise<PlaybookEntry[]> {
  const builtins: PlaybookEntry[] = BUILTIN_PLAYBOOKS.map((t) => ({
    template: t,
    source: "builtin",
  }));
  const r = await commands.listPlaybooks();
  if (r.status === "error") {
    console.warn("[playbooks] list failed:", r.error);
    return builtins;
  }
  const saved: PlaybookEntry[] = [];
  for (const stored of r.data) {
    const parsed = parsePlaybookJson(stored.json);
    if ("ok" in parsed) {
      // The on-disk filename is authoritative for the id — overwrite
      // any id field in the body so manual JSON edits to that field
      // don't desync from the filesystem.
      saved.push({
        template: { ...parsed.ok, id: stored.id },
        source: "saved",
      });
    } else {
      console.warn(
        `[playbooks] skipping malformed saved playbook ${stored.id}:`,
        parsed.err,
      );
    }
  }
  return [...builtins, ...saved];
}

/// Persist a playbook to disk. The Rust store re-validates the JSON
/// shape and the id; errors come back as the IPC `error` field.
export async function savePlaybook(
  template: PlaybookTemplate,
): Promise<{ ok: true } | { err: string }> {
  const json = JSON.stringify(template, null, 2);
  const r = await commands.savePlaybook(template.id, json);
  return r.status === "ok" ? { ok: true } : { err: r.error };
}

/// Idempotent delete — missing files report success (Rust side).
export async function deletePlaybook(
  id: string,
): Promise<{ ok: true } | { err: string }> {
  const r = await commands.deletePlaybook(id);
  return r.status === "ok" ? { ok: true } : { err: r.error };
}

/// Strict schema validator for imported / disk-loaded playbook JSON.
/// Returns the parsed `PlaybookTemplate` on success, or a single
/// human-readable error on failure. Forward-compat: unknown
/// `MockScript` values are accepted (the runner has a `vendorOf`
/// fallback for them).
export function parsePlaybookJson(
  input: string,
): { ok: PlaybookTemplate } | { err: string } {
  let raw: unknown;
  try {
    raw = JSON.parse(input);
  } catch (e) {
    return {
      err: `not valid JSON: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
  if (!isObject(raw)) return { err: "playbook must be a JSON object" };

  if (typeof raw.id !== "string" || raw.id.length === 0) {
    return { err: "playbook.id must be a non-empty string" };
  }
  if (typeof raw.name !== "string" || raw.name.length === 0) {
    return { err: "playbook.name must be a non-empty string" };
  }
  if (typeof raw.description !== "string") {
    return { err: "playbook.description must be a string" };
  }

  if (!isObject(raw.squad)) return { err: "playbook.squad must be an object" };
  if (typeof raw.squad.name !== "string" || raw.squad.name.length === 0) {
    return { err: "playbook.squad.name must be a non-empty string" };
  }
  if (typeof raw.squad.color !== "string") {
    return { err: "playbook.squad.color must be a string" };
  }
  if (!(SQUAD_COLORS as readonly string[]).includes(raw.squad.color)) {
    return {
      err: `playbook.squad.color invalid: ${raw.squad.color} (must be one of ${SQUAD_COLORS.join(", ")})`,
    };
  }

  if (!Array.isArray(raw.members)) {
    return { err: "playbook.members must be an array" };
  }
  if (raw.members.length === 0) {
    return { err: "playbook.members cannot be empty" };
  }
  const members: PlaybookMember[] = [];
  for (let i = 0; i < raw.members.length; i++) {
    const result = parseMember(raw.members[i], i);
    if ("err" in result) return result;
    members.push(result.ok);
  }

  return {
    ok: {
      id: raw.id,
      name: raw.name,
      description: raw.description,
      squad: {
        name: raw.squad.name,
        color: raw.squad.color as SquadColor,
      },
      members,
    },
  };
}

function parseMember(
  m: unknown,
  idx: number,
): { ok: PlaybookMember } | { err: string } {
  if (!isObject(m)) return { err: `members[${idx}] must be an object` };
  if (typeof m.role !== "string" || m.role.length === 0) {
    return { err: `members[${idx}].role must be a non-empty string` };
  }
  if (!isObject(m.spawn)) {
    return { err: `members[${idx}].spawn must be an object` };
  }
  const spawnResult = parseSpawn(m.spawn, idx);
  if ("err" in spawnResult) return spawnResult;

  if (m.isLead !== undefined && typeof m.isLead !== "boolean") {
    return { err: `members[${idx}].isLead must be boolean if present` };
  }
  return {
    ok: {
      role: m.role,
      spawn: spawnResult.ok,
      isLead: m.isLead as boolean | undefined,
    },
  };
}

function parseSpawn(
  spawn: Record<string, unknown>,
  idx: number,
): { ok: PlaybookMemberSpawn } | { err: string } {
  const kind = spawn.kind;
  if (kind === "mock") {
    if (typeof spawn.script !== "string") {
      return { err: `members[${idx}].spawn.script must be a string` };
    }
    return {
      ok: { kind: "mock", script: spawn.script as PlaybookMemberSpawn extends { kind: "mock"; script: infer S } ? S : never },
    };
  }
  if (kind === "claude") {
    if (typeof spawn.prompt !== "string") {
      return { err: `members[${idx}].spawn.prompt must be a string` };
    }
    if (typeof spawn.cwd !== "string") {
      return { err: `members[${idx}].spawn.cwd must be a string` };
    }
    if (
      spawn.maxTurns !== undefined &&
      spawn.maxTurns !== null &&
      typeof spawn.maxTurns !== "number"
    ) {
      return {
        err: `members[${idx}].spawn.maxTurns must be a number, null, or absent`,
      };
    }
    return {
      ok: {
        kind: "claude",
        prompt: spawn.prompt,
        cwd: spawn.cwd,
        maxTurns: (spawn.maxTurns as number | null | undefined) ?? null,
      },
    };
  }
  if (kind === "codex") {
    if (typeof spawn.prompt !== "string") {
      return { err: `members[${idx}].spawn.prompt must be a string` };
    }
    if (typeof spawn.cwd !== "string") {
      return { err: `members[${idx}].spawn.cwd must be a string` };
    }
    return { ok: { kind: "codex", prompt: spawn.prompt, cwd: spawn.cwd } };
  }
  if (kind === "gemini") {
    if (typeof spawn.prompt !== "string") {
      return { err: `members[${idx}].spawn.prompt must be a string` };
    }
    if (typeof spawn.cwd !== "string") {
      return { err: `members[${idx}].spawn.cwd must be a string` };
    }
    return { ok: { kind: "gemini", prompt: spawn.prompt, cwd: spawn.cwd } };
  }
  return {
    err: `members[${idx}].spawn.kind must be "mock" / "claude" / "codex" / "gemini" (got ${JSON.stringify(kind)})`,
  };
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
