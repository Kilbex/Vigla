import type { Vendor } from "../bindings";
import type { SquadColor } from "../store/types";

/// A playbook member's spawn definition. Discriminated union mirroring the
/// standalone-worker IPC commands supported by the playbook runner.
export type PlaybookMemberSpawn =
  | { kind: "mock"; script: MockScript }
  | { kind: "claude"; prompt: string; cwd: string; maxTurns?: number | null }
  | { kind: "codex"; prompt: string; cwd: string }
  | { kind: "gemini"; prompt: string; cwd: string };

export type MockScript =
  | "claude_happy"
  | "codex_blocked"
  | "gemini_happy"
  | "gemini_blocked"
  | "gemini_failed"
  | "gemini_terminal";

/// One member of a playbook team. The `role` is a human label
/// ("implementer", "reviewer", "lead") used as the worker's task title.
/// `isLead` controls the squad-lead chevron; it grants no extra runtime
/// authority.
export interface PlaybookMember {
  role: string;
  spawn: PlaybookMemberSpawn;
  isLead?: boolean;
}

/// A reusable team-shape definition. Disk-backed user playbooks and built-ins
/// share this shape.
export interface PlaybookTemplate {
  id: string;
  name: string;
  description: string;
  squad: {
    name: string;
    color: SquadColor;
  };
  members: PlaybookMember[];
}

/// Mock scripts simulate specific vendors. The runner uses this map
/// to call `registerWorker` with the *implied* vendor so a "Mixed
/// Demo" playbook displays a real-looking multi-vendor squad in the
/// UI (even though every worker is a mock CLI under the hood).
export const MOCK_VENDOR_MAP: Record<MockScript, Vendor> = {
  claude_happy: "claude",
  codex_blocked: "codex",
  gemini_happy: "gemini",
  gemini_blocked: "gemini",
  gemini_failed: "gemini",
  gemini_terminal: "gemini",
};

/// Returns the vendor a member's spawn implies, used by the runner
/// to populate the WorkerSnapshot's `vendor` field at registration.
///
/// The mock branch falls back to `"mock"` if `spawn.script` is not in
/// `MOCK_VENDOR_MAP`. TypeScript's exhaustive `Record<MockScript,...>`
/// catches in-tree drift at compile time; the runtime fallback exists
/// for the Step-22 path where disk-backed playbooks may ship a
/// future MockScript value the build hasn't seen.
export function vendorOf(spawn: PlaybookMemberSpawn): Vendor {
  switch (spawn.kind) {
    case "mock":
      return MOCK_VENDOR_MAP[spawn.script] ?? "mock";
    case "claude":
      return "claude";
    case "codex":
      return "codex";
    case "gemini":
      return "gemini";
  }
}
