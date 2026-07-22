// Browser-side mock of `@tauri-apps/api/core`. Aliased in via
// `app/vite.config.ts` when `VITE_VIGLA_E2E=1` so the real
// Tauri IPC layer is replaced by an in-process invoke that the
// Playwright tests can introspect.
//
// The mock records every invoke call on `window.__viglaE2e`
// and answers the commands the browser suites need:
//
//   - list_recent_missions      → one completed mission row
//   - revert_mission            → restored_sha + pre_merge_tag
//   - surface_inbox_notification→ recorded; returns null
//   - mission_event_visibility  → category map mirroring Rust
//   - check_cli_auth            → ready CLI rows for deploy-panel render
//   - health_check / get_worker_info → stable browser-only identity data
//
// Anything else returns `null` so accidental calls don't crash
// the surface under test.

type InvokeArgs = Record<string, unknown> | undefined;

interface E2eHandle {
  invokeCalls: { cmd: string; args: InvokeArgs }[];
  notifications: { title: string; body: string }[];
  revertCalls: { missionId: string }[];
  listenerCount: number;
  emitMissionEvent: (payload: unknown) => Promise<void>;
  emitWorkerEvent: (payload: unknown) => Promise<void>;
  setRecentMissions: (rows: unknown[]) => void;
  setRevertResponse: (resp: { restored_sha: string; pre_merge_tag: string }) => void;
  setVisibility: (
    rules: (kind: { type: string; payload?: unknown }) => unknown,
  ) => void;
}

declare global {
  interface Window {
    __viglaE2e: E2eHandle;
    __viglaE2eListeners?: Map<string, Set<(payload: unknown) => void>>;
  }
}

const DEFAULT_HISTORY_ROW = {
  mission_id: "msn-e2e-0001",
  audit_overall: 0.92,
  tier: "standard",
  created_at: new Date().toISOString(),
  reverted: false,
  status: "merged",
  target_ref: "main",
};

const DEFAULT_REVERT = {
  restored_sha: "deadbeefcafe1234",
  pre_merge_tag: "vigla/revert/msn-e2e-0001/before/main",
};

const DEFAULT_CLI_AUTH = [
  {
    vendor: "claude",
    display_name: "Claude",
    binary: "claude",
    binary_present: true,
    state: "ready",
    detail: "Claude CLI is logged in.",
    login_command: "claude login",
    docs_url: "https://docs.anthropic.com/",
  },
  {
    vendor: "codex",
    display_name: "Codex",
    binary: "codex",
    binary_present: true,
    state: "ready",
    detail: "Codex CLI is logged in.",
    login_command: "codex login",
    docs_url: "https://developers.openai.com/codex/",
  },
  {
    vendor: "antigravity",
    display_name: "Antigravity",
    binary: "agy",
    binary_present: true,
    state: "ready",
    detail: "Antigravity CLI is ready.",
    login_command: "agy auth login",
    docs_url: "https://developers.google.com/gemini-code-assist/docs",
  },
  {
    vendor: "gemini",
    display_name: "Gemini (legacy)",
    binary: "gemini",
    binary_present: false,
    state: "missing_binary",
    detail: "Legacy and enterprise compatibility only.",
    login_command: "gemini auth login",
    docs_url: "https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals",
  },
];

const MOCK_BOOT_TIME = Date.now();

function identityFor(workerId: string) {
  const vendorMatch = /^wkr-([a-z][a-z0-9_-]*?)-0*[0-9]+$/i.exec(workerId);
  const candidate = vendorMatch?.[1]?.toLowerCase() ?? "mock";
  const vendor = [
    "claude",
    "codex",
    "gemini",
    "antigravity",
    "kiro",
    "copilot",
    "opencode",
  ].includes(candidate)
    ? candidate
    : "mock";
  const models: Record<string, string | null> = {
    claude: "claude-sonnet",
    codex: "gpt-codex",
    antigravity: "antigravity-agent",
    gemini: "gemini-legacy",
    kiro: null,
    copilot: null,
    opencode: null,
    mock: null,
  };
  return {
    id: workerId,
    name: workerId,
    vendor,
    cli_binary: "recorded-events",
    cli_version: null,
    cwd: "/read-only/demo",
    model: models[vendor],
    spawned_at: new Date(MOCK_BOOT_TIME).toISOString(),
    ended_at: null,
  };
}

function defaultVisibility(kind: { type: string; payload?: unknown }): unknown {
  switch (kind.type) {
    case "mission.completed":
    case "mission.merge_resolved":
    case "mission.completion_verdict_rendered":
      return { kind: "inbox", inbox_kind: "completion", severity: "info" };
    case "mission.aborted":
      return { kind: "inbox", inbox_kind: "escalation", severity: "warning" };
    case "boundary.side_effect_logged":
      return { kind: "inbox", inbox_kind: "side_effect", severity: "warning" };
    case "plan.proposed":
      return { kind: "inbox", inbox_kind: "escalation", severity: "action_required" };
    case "arbiter.decided": {
      const payload = (kind.payload ?? {}) as { bound?: unknown };
      if (payload.bound != null) {
        return { kind: "inbox", inbox_kind: "escalation", severity: "action_required" };
      }
      return { kind: "internal" };
    }
    case "supervisor.audit_completed":
      return { kind: "inbox", inbox_kind: "completion", severity: "info" };
    default:
      return { kind: "internal" };
  }
}

function initHandle(): E2eHandle {
  if (window.__viglaE2e) return window.__viglaE2e;
  const state: {
    recent: unknown[];
    revert: { restored_sha: string; pre_merge_tag: string };
    visibility: (kind: { type: string; payload?: unknown }) => unknown;
  } = {
    recent: [DEFAULT_HISTORY_ROW],
    revert: DEFAULT_REVERT,
    visibility: defaultVisibility,
  };

  const dispatch = (channel: string, payload: unknown) => {
    const map = window.__viglaE2eListeners;
    if (!map) return;
    const subs = map.get(channel);
    if (!subs) return;
    for (const fn of subs) {
      try {
        fn(payload);
      } catch {
        // listener threw — ignore so emit() resolves cleanly.
      }
    }
  };

  const handle: E2eHandle = {
    invokeCalls: [],
    notifications: [],
    revertCalls: [],
    listenerCount: 0,
    emitMissionEvent: async (payload) => {
      dispatch("mission-event-dto", { event: "mission-event-dto", payload, id: 0 });
    },
    emitWorkerEvent: async (payload) => {
      dispatch("worker-event", { event: "worker-event", payload, id: 0 });
    },
    setRecentMissions: (rows) => {
      state.recent = rows;
    },
    setRevertResponse: (resp) => {
      state.revert = resp;
    },
    setVisibility: (rules) => {
      state.visibility = rules;
    },
  };

  (handle as unknown as { _state: typeof state })._state = state;
  window.__viglaE2e = handle;
  return handle;
}

export async function invoke<T = unknown>(
  cmd: string,
  args?: InvokeArgs,
): Promise<T> {
  const handle = initHandle();
  handle.invokeCalls.push({ cmd, args });
  const state = (handle as unknown as {
    _state: {
      recent: unknown[];
      revert: { restored_sha: string; pre_merge_tag: string };
      visibility: (k: { type: string; payload?: unknown }) => unknown;
    };
  })._state;

  switch (cmd) {
    case "health_check":
      return {
        version: "0.1.0-demo",
        uptime_ms: Date.now() - MOCK_BOOT_TIME,
      } as T;
    case "startup_status":
      return { phase: "ready", error: null } as T;
    case "check_cli_auth":
      return DEFAULT_CLI_AUTH as T;
    case "list_recent_missions":
      return state.recent as T;
    case "list_recent_workers":
    case "replay_worker_events_page":
      return [] as T;
    case "get_worker_info": {
      const { workerId } = (args ?? {}) as { workerId: string };
      return identityFor(workerId) as T;
    }
    case "revert_mission": {
      const { missionId } = (args ?? {}) as { missionId: string };
      handle.revertCalls.push({ missionId });
      return state.revert as T;
    }
    case "surface_inbox_notification": {
      const { title, body } = (args ?? {}) as { title: string; body: string };
      handle.notifications.push({ title, body });
      return null as T;
    }
    case "mission_event_visibility": {
      const { event } = (args ?? {}) as {
        event: { type: string; payload?: unknown };
      };
      return state.visibility(event) as T;
    }
    default:
      return null as T;
  }
}

// `@tauri-apps/api/core` also exports a handful of helpers we
// don't exercise; expose no-op stubs so accidental imports don't
// blow up the bundle.
export const convertFileSrc = (path: string): string => path;
export const transformCallback = <T>(cb?: (v: T) => void): number => {
  if (cb) cb(undefined as unknown as T);
  return 0;
};
export const Channel = class {
  onmessage: ((m: unknown) => void) | null = null;
};
export const PluginListener = class {
  unregister(): void {
    /* noop */
  }
};
export const SERIALIZE_TO_IPC_FN = Symbol("noop");

// Pre-init on module load so window.__viglaE2e exists before
// any test code runs (Playwright `expect.poll` etc. can read it
// even if the app hasn't called invoke yet).
if (typeof window !== "undefined") initHandle();
