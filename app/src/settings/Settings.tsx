import { useCallback, useEffect, useRef, useState } from "react";
import { useDialogFocus } from "../useDialogFocus";
import {
  commands,
  type AppSettingsDto,
  type CliAuthState,
  type CliAuthStatusDto,
  type MissionSpec,
} from "../bindings";
import { useMissionsStore } from "../missions/store";
import { useOpsStore } from "../store";
import { useNotifyOnCompletion, usePlanMode, useShowAllEvents } from "./preferences";

interface SettingsProps {
  open: boolean;
  onClose: () => void;
}

const MOCK_SCRIPTS = [
  "claude_happy",
  "codex_blocked",
  "gemini_happy",
] as const;
type MockScript = (typeof MOCK_SCRIPTS)[number];

const SHOW_DEV_TOOLS =
  import.meta.env.DEV || import.meta.env.VITE_VIGLA_E2E === "1";

export default function Settings({ open, onClose }: SettingsProps) {
  const dialogRef = useRef<HTMLElement>(null);
  // FE-A2: focus into the modal on open, trap Tab within it, restore focus to
  // the trigger on close.
  useDialogFocus(open, dialogRef);
  const [data, setData] = useState<AppSettingsDto | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [mockBusy, setMockBusy] = useState<MockScript | null>(null);
  const [quotaBusy, setQuotaBusy] = useState(false);
  const [devStatus, setDevStatus] = useState<string | null>(null);
  const [showAllEvents, setShowAllEvents] = useShowAllEvents();
  const [notifyOnCompletion, setNotifyOnCompletion] = useNotifyOnCompletion();
  const [planMode, setPlanMode] = usePlanMode();

  const reset = useOpsStore((s) => s.reset);
  const setCurrentRepoCwd = useMissionsStore((s) => s.setCurrentRepoCwd);

  const loadSettings = useCallback(async (cancelled?: () => boolean) => {
    setData(null);
    setErr(null);
    try {
      const s = await commands.appSettings();
      if (cancelled?.()) return;
      setData(s);
      setErr(null);
    } catch (e) {
      if (cancelled?.()) return;
      setErr(String(e));
      setData(null);
    }
  }, []);

  const runMock = async (script: MockScript) => {
    setMockBusy(script);
    setDevStatus(null);
    try {
      const r = await commands.startMockWorker(script, 1.0);
      setDevStatus(
        r.status === "ok" ? `started ${script}` : `error: ${r.error}`,
      );
    } catch (e) {
      setDevStatus(`error: ${e}`);
    } finally {
      setMockBusy(null);
    }
  };

  const runL1QuotaMission = async () => {
    if (!data?.l1_quota_mock_enabled) {
      setDevStatus("error: L1 quota mock is not enabled");
      return;
    }
    const repoRoot = data.configured_repo_root;
    if (!repoRoot) {
      setDevStatus("error: VIGLA_REPO_ROOT is not configured to a Git repository");
      return;
    }
    setQuotaBusy(true);
    setDevStatus(null);
    const spec: MissionSpec = {
      title: "L1 row-4 quota observation",
      objective:
        "Append the current ISO-8601 timestamp as a new line at the end of notes/quota-pings.md. If the file or its parent directory does not exist, create them first. Do not touch any other file.",
      target_ref: "main",
      tests: null,
      supervisor_model: "claude",
      worker_model: "claude_quota_exhausted",
      worker_count: 1,
      confirm_plan: null,
      scope_paths: ["notes/quota-pings.md"],
    };
    try {
      const r = await commands.startMission(spec, repoRoot);
      if (r.status === "error") {
        setDevStatus(`error: ${r.error}`);
        return;
      }
      setCurrentRepoCwd(repoRoot);
      setDevStatus(`started L1 quota mission ${r.data}`);
    } catch (e) {
      setDevStatus(`error: ${e}`);
    } finally {
      setQuotaBusy(false);
    }
  };

  const resetRoom = () => {
    reset();
    setDevStatus("room reset");
  };

  const openLogin = async (vendor: string) => {
    setDevStatus(null);
    try {
      const r = await commands.openCliLogin(vendor);
      setDevStatus(
        r.status === "ok"
          ? `opened ${vendor} login`
          : `error: ${r.error}`,
      );
    } catch (e) {
      setDevStatus(`error: ${e}`);
    }
  };

  const openDocs = async (vendor: string) => {
    setDevStatus(null);
    try {
      const r = await commands.openCliAuthDocs(vendor);
      setDevStatus(
        r.status === "ok"
          ? `opened ${vendor} docs`
          : `error: ${r.error}`,
      );
    } catch (e) {
      setDevStatus(`error: ${e}`);
    }
  };

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // Audit r5 polish — clear the OTHER side of the success/error
    // pair on every transition. Without this, a successful first
    // open + a rejecting subsequent open would render the stale
    // success body alongside the new error banner, contradicting
    // each other on screen.
    loadSettings(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, [open, loadSettings]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // Settings is the topmost dismissable layer when open. Capture-
        // phase + stopImmediatePropagation makes sure the Drawer's
        // bubble-phase Esc handler doesn't also fire and close the
        // drawer behind us in the same keystroke.
        e.stopImmediatePropagation();
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [open, onClose]);

  if (!open) return null;

  return (
    <>
      <button
        className="settings-scrim"
        onClick={onClose}
        aria-label="close settings"
      />
      <section
        className="settings"
        role="dialog"
        aria-modal="true"
        aria-label="Vigla settings"
        ref={dialogRef}
      >
        <header className="settings-head">
          <span className="settings-title">SETTINGS</span>
          <button className="settings-close" onClick={onClose} aria-label="close">
            esc
          </button>
        </header>
        {err ? <div className="settings-error">{err}</div> : null}
        {data ? (
          <div className="settings-body">
            <section className="settings-section settings-section--compact">
              <h3 className="settings-section-title">Environment</h3>
              <Row
                label="App version"
                value={`v${data.version}`}
                accent="muted"
              />
              <Row
                label="Database"
                value={data.db_path}
                mono
              />
              <Row
                label="Mock harness"
                value={data.mock_harness_path}
                mono
                status={data.mock_harness_present ? "ok" : "warn"}
              />
              <Row
                label="Claude CLI"
                value={data.claude_present ? "available on PATH" : "not detected"}
                status={data.claude_present ? "ok" : "muted"}
              />
              <Row
                label="Codex CLI"
                value={data.codex_present ? "available on PATH" : "not detected"}
                status={data.codex_present ? "ok" : "muted"}
              />
              <Row
                label="Antigravity CLI"
                value={
                  data.antigravity_present
                    ? "available on PATH"
                    : "not detected"
                }
                status={data.antigravity_present ? "ok" : "muted"}
              />
              <Row
                label="Kiro CLI"
                value={data.kiro_present ? "available on PATH" : "not detected"}
                status={data.kiro_present ? "ok" : "muted"}
              />
              <Row
                label="GitHub Copilot CLI"
                value={
                  data.copilot_present ? "available on PATH" : "not detected"
                }
                status={data.copilot_present ? "ok" : "muted"}
              />
              <Row
                label="Gemini CLI (legacy)"
                value={
                  data.gemini_present ? "available on PATH" : "not detected"
                }
                status={data.gemini_present ? "ok" : "muted"}
              />
            </section>
            <div className="settings-divider" />
            <section className="settings-section settings-section--compact">
              <div className="settings-section-head">
                <h3 className="settings-section-title">CLI auth</h3>
                <button
                  type="button"
                  className="settings-link-button"
                  onClick={() => loadSettings()}
                >
                  Refresh
                </button>
              </div>
              <div className="settings-auth-list">
                {data.cli_auth.map((status) => (
                  <AuthRow
                    key={status.vendor}
                    status={status}
                    onLogin={() => openLogin(status.vendor)}
                    onDocs={() => openDocs(status.vendor)}
                  />
                ))}
              </div>
            </section>
            <div className="settings-divider" />
            <section className="settings-section settings-section--compact">
              <h3 className="settings-section-title">Shortcuts</h3>
              <Row label={<><kbd className="kbd">⌘</kbd><kbd className="kbd">1</kbd></>} value="Inbox surface (default)" />
              <Row label={<><kbd className="kbd">⌘</kbd><kbd className="kbd">2</kbd></>} value="Ops Room (enables Show all events)" />
              <Row label={<><kbd className="kbd">⌘</kbd><kbd className="kbd">3</kbd></>} value="Recent missions history" />
              {SHOW_DEV_TOOLS ? (
                <Row
                  label={
                    <>
                      <kbd className="kbd">1</kbd> / <kbd className="kbd">2</kbd> / <kbd className="kbd">3</kbd>
                    </>
                  }
                  value="Spawn Claude / Codex / legacy Gemini mock"
                />
              ) : null}
              <Row label={<><kbd className="kbd">⌘</kbd><kbd className="kbd">H</kbd></>} value="Toggle replay / history mode" />
              <Row label={<><kbd className="kbd">⌘</kbd><kbd className="kbd">,</kbd></>} value="Open this dialog" />
              <Row label="Comms feed" value="Deploy workers — multi-vendor launch" />
              <Row label={<kbd className="kbd">Esc</kbd>} value="Close drawer / settings / replay" />
            </section>
            <div className="settings-divider" />
            <section className="settings-section">
              <h3 className="settings-section-title">Preferences</h3>
              <label className="settings-toggle">
                <input
                  type="checkbox"
                  checked={showAllEvents}
                  onChange={(e) => setShowAllEvents(e.target.checked)}
                />
                <span className="settings-toggle-label">Show all events</span>
                <span className="settings-toggle-detail">
                  When on, the right rail shows the legacy CommsFeed
                  (worker progress, plan churn, every supervisor
                  transition). When off (default), only the Inbox
                  shows — completions, escalations, and side effects.
                </span>
              </label>
              <label className="settings-toggle">
                <input
                  type="checkbox"
                  checked={notifyOnCompletion}
                  onChange={(e) => setNotifyOnCompletion(e.target.checked)}
                />
                <span className="settings-toggle-label">
                  Notify on completion
                </span>
                <span className="settings-toggle-detail">
                  macOS banner when a mission finishes while the
                  Vigla window is unfocused.
                </span>
              </label>
              <fieldset className="settings-radio-group">
                <legend className="settings-toggle-label">Default plan mode</legend>
                <span className="settings-toggle-detail">
                  Direct (default) lets the supervisor proceed once
                  the four-bound envelope is clear. Review pauses for
                  one explicit user confirmation before workers spawn.
                  Either way, an envelope-fit overrun forces a
                  pause — the supervisor's self-assessment overrides
                  this preference.
                </span>
                <div className="settings-radio-options">
                  <label className="settings-radio">
                    <input
                      type="radio"
                      name="plan-mode"
                      value="direct"
                      checked={planMode === "direct"}
                      onChange={() => setPlanMode("direct")}
                    />
                    <span className="settings-radio-label">Direct</span>
                  </label>
                  <label className="settings-radio">
                    <input
                      type="radio"
                      name="plan-mode"
                      value="review"
                      checked={planMode === "review"}
                      onChange={() => setPlanMode("review")}
                    />
                    <span className="settings-radio-label">Review</span>
                  </label>
                </div>
              </fieldset>
            </section>
            {SHOW_DEV_TOOLS ? (
              <>
                <div className="settings-divider" />
                <section className="settings-section settings-section--compact">
                  <h3 className="settings-section-title">Developer</h3>
                  <div className="settings-row">
                    <span className="settings-row-label">Mock spawn</span>
                    <div className="settings-dev-actions">
                      {MOCK_SCRIPTS.map((s) => (
                        <button
                          key={s}
                          className="spawn-btn"
                          onClick={() => runMock(s)}
                          disabled={mockBusy === s}
                          aria-disabled={mockBusy === s}
                        >
                          {mockBusy === s ? "starting…" : s}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="settings-row">
                    <span className="settings-row-label">Room</span>
                    <div className="settings-dev-actions">
                      <button
                        className="spawn-btn spawn-btn-clear"
                        onClick={resetRoom}
                        aria-label="reset operations room"
                      >
                        reset room
                      </button>
                    </div>
                  </div>
                  <div className="settings-row">
                    <span className="settings-row-label">L1 quota</span>
                    <div className="settings-dev-actions">
                      <button
                        className="spawn-btn"
                        onClick={runL1QuotaMission}
                        disabled={
                          quotaBusy ||
                          !data.l1_quota_mock_enabled ||
                          !data.configured_repo_root
                        }
                        aria-disabled={
                          quotaBusy ||
                          !data.l1_quota_mock_enabled ||
                          !data.configured_repo_root
                        }
                        title={
                          !data.l1_quota_mock_enabled
                            ? "Run scripts/observe-quota.sh to enable"
                            : data.configured_repo_root ??
                              "Set VIGLA_REPO_ROOT to a Git repository"
                        }
                      >
                        {quotaBusy ? "starting…" : "start quota mission"}
                      </button>
                    </div>
                  </div>
                  {devStatus ? (
                    <div
                      className={
                        "comms-status" +
                        (devStatus.startsWith("error")
                          ? " comms-status-error"
                          : "")
                      }
                      role="status"
                    >
                      {devStatus}
                    </div>
                  ) : null}
                </section>
              </>
            ) : null}
          </div>
        ) : (
          <SettingsSkeleton />
        )}
      </section>
    </>
  );
}

function SettingsSkeleton() {
  return (
    <div className="settings-body" role="status" aria-label="Loading settings">
      {Array.from({ length: 5 }, (_, index) => (
        <div key={index} className="settings-skeleton-row">
          <span className="settings-skeleton-label" />
          <span className="settings-skeleton-value" />
        </div>
      ))}
    </div>
  );
}

interface RowProps {
  label: string | React.ReactNode;
  value: string;
  mono?: boolean;
  accent?: "muted";
  status?: "ok" | "warn" | "muted";
}

function Row({ label, value, mono, accent, status }: RowProps) {
  const valueClass = [
    "settings-row-value",
    mono ? "settings-row-value--mono" : "",
    accent === "muted" ? "settings-row-value--muted" : "",
    status === "ok" ? "settings-row-value--ok" : "",
    status === "warn" ? "settings-row-value--warn" : "",
    status === "muted" ? "settings-row-value--muted" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="settings-row">
      <span className="settings-row-label">{label}</span>
      <span className={valueClass}>{value}</span>
    </div>
  );
}

function AuthRow({
  status,
  onLogin,
  onDocs,
}: {
  status: CliAuthStatusDto;
  onLogin: () => void;
  onDocs: () => void;
}) {
  const label = authStateLabel(status.state);
  const statusClass = [
    "settings-auth-state",
    status.state === "ready" ? "settings-auth-state--ready" : "",
    status.state === "not_logged_in" ? "settings-auth-state--warn" : "",
    status.state === "missing_cli" ? "settings-auth-state--muted" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="settings-auth-row">
      <div className="settings-auth-main">
        <span className="settings-auth-name">{status.display_name}</span>
        <span className={statusClass}>{label}</span>
        <span className="settings-auth-detail">{status.detail}</span>
      </div>
      <div className="settings-auth-actions">
        {status.state !== "ready" && status.binary_present ? (
          <button
            type="button"
            className="settings-link-button"
            onClick={onLogin}
            aria-label={`log in to ${status.display_name}`}
          >
            Login
          </button>
        ) : null}
        <button
          type="button"
          className="settings-link-button"
          onClick={onDocs}
          aria-label={`open ${status.display_name} auth docs`}
        >
          Docs
        </button>
      </div>
    </div>
  );
}

function authStateLabel(state: CliAuthState): string {
  switch (state) {
    case "ready":
      return "Logged in";
    case "missing_cli":
      return "CLI missing";
    case "not_logged_in":
      return "Login needed";
    case "unknown":
      return "Unknown";
  }
}
