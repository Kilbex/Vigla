// MSV U1 (refined) — unified team-launch surface.
//
// Default surface: objective + project folder + Start. That's it.
// Advanced disclosure (collapsed) carries the power knobs:
//   - SUPERVISOR model (default Claude)
//   - WORKER COUNT    (default "auto" → supervisor decides)
//   - WORKER CLI roster (one vendor CLI choice and one
//     applied CLI model per worker when a concrete worker count is
//     selected)
//
// When WORKER COUNT is "auto", the request omits worker_model/count
// (null in MissionSpec). The orchestrator then routes workers by
// task role and lets the supervisor decompose the needed task count.

import { useCallback, useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { commands, type CliAuthStatusDto } from "../bindings";
import { useMissionsStore } from "../missions/store";
import { selectCanStartMission } from "../missions/store";
import { usePlanMode, type PlanMode } from "../settings/preferences";
import WorkerCliRow from "./WorkerCliRow";
import {
  COUNT_LABEL,
  DEFAULT_WORKER_MODELS,
  MODEL_LABEL,
  SUPERVISOR_OPTIONS,
  WORKER_COUNT_OPTIONS,
  encodeWorkerModelRoster,
  fillWorkerCliModels,
  fillWorkerModels,
  normalizeWorkerCliModel,
  workerCountNumber,
  type SupervisorVendor,
  type WorkerCliModelValue,
  type WorkerCountChoice,
  type WorkerVendor,
} from "./deploy-models";
import {
  initialCount,
  initialScopePathsText,
  initialSupervisor,
  initialWorkerCliModels,
  initialWorkerModels,
  loadPrefs,
  savePrefs,
} from "./deploy-preferences";
import { parseScopePaths } from "./deploy-scope";

export { parseScopePaths } from "./deploy-scope";

// Budget gate retired in Task 14; the dollar-budget UI field was removed in
// Task 16 — the arbiter governs spend implicitly via rework attempts.

export default function DeployPanel() {
  const [initialDeployPrefs] = useState(() => {
    const prefs = loadPrefs();
    const workerModels = initialWorkerModels(prefs);
    return {
      prefs,
      workerModels,
      workerCliModels: initialWorkerCliModels(prefs, workerModels),
    };
  });
  const [objective, setObjective] = useState("");
  const [cwd, setCwd] = useState(() => initialDeployPrefs.prefs.cwd ?? "");
  const [supervisorModel] = useState<SupervisorVendor>(() =>
    initialSupervisor(initialDeployPrefs.prefs),
  );
  const [workerModels, setWorkerModels] = useState<WorkerVendor[]>(() =>
    initialDeployPrefs.workerModels,
  );
  const [workerCliModels, setWorkerCliModels] = useState<
    WorkerCliModelValue[]
  >(() =>
    initialDeployPrefs.workerCliModels,
  );
  const [workerCount, setWorkerCount] = useState<WorkerCountChoice>(() =>
    initialCount(initialDeployPrefs.prefs),
  );
  // QC-3: replaces the QC-2 confirmPlan checkbox. Defaults to the
  // user's Settings preference; per-deploy override is local-only
  // and not persisted (the Settings panel is authoritative for the
  // default).
  const [planMode, setPlanMode] = usePlanMode();
  const [scopePathsText, setScopePathsText] = useState<string>(() =>
    initialScopePathsText(initialDeployPrefs.prefs),
  );
  const [authStatuses, setAuthStatuses] = useState<CliAuthStatusDto[]>([]);
  const [authBusy, setAuthBusy] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [loginBusy, setLoginBusy] = useState<WorkerVendor | null>(null);

  // Persist preferences whenever the user changes any of the sticky
  // fields. Debounce isn't necessary — these update on discrete events
  // (folder pick, dropdown change), not on every keystroke. The
  // scope-paths textarea DOES update on every keystroke, but localStorage
  // writes at typing speed are cheap (~tens of µs for a few-kB string).
  useEffect(() => {
    savePrefs({
      cwd,
      supervisorModel,
      workerModels,
      workerCliModels,
      workerCount,
      scopePathsText,
    });
  }, [
    cwd,
    supervisorModel,
    workerModels,
    workerCliModels,
    workerCount,
    scopePathsText,
  ]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [scopePathErrors, setScopePathErrors] = useState<string[]>([]);

  const canStart = useMissionsStore(selectCanStartMission);
  const setCurrentRepoCwd = useMissionsStore((s) => s.setCurrentRepoCwd);
  const selectedWorkerCount = workerCountNumber(workerCount);
  const authByVendor = useMemo(
    () => new Map(authStatuses.map((s) => [s.vendor, s])),
    [authStatuses],
  );

  const refreshCliAuth = useCallback(async () => {
    setAuthBusy(true);
    try {
      const statuses = await commands.checkCliAuth();
      setAuthStatuses(Array.isArray(statuses) ? statuses : []);
      setAuthError(null);
    } catch (e) {
      setAuthError(e instanceof Error ? e.message : String(e));
    } finally {
      setAuthBusy(false);
    }
  }, []);

  useEffect(() => {
    void refreshCliAuth();
  }, [refreshCliAuth]);

  const setWorkerModelAt = (index: number, model: WorkerVendor) => {
    // Compute the updated vendor array once and reuse it for CLI-model
    // normalization. Previously the CLI updater closed over the STALE
    // `workerModels`, so changing one slot's vendor re-normalized the other
    // slots' CLI models against the OLD vendors (FE-3).
    const nextModels = fillWorkerModels(workerModels);
    nextModels[index] = model;
    setWorkerModels(nextModels);
    setWorkerCliModels((prev) => {
      const next = fillWorkerCliModels(prev, nextModels);
      next[index] = normalizeWorkerCliModel(model, next[index]);
      return next;
    });
  };

  const setWorkerCliModelAt = (
    index: number,
    cliModel: WorkerCliModelValue,
  ) => {
    setWorkerCliModels((prev) => {
      const next = fillWorkerCliModels(prev, workerModels);
      const vendor =
        workerModels[index] ??
        DEFAULT_WORKER_MODELS[index % DEFAULT_WORKER_MODELS.length];
      next[index] = normalizeWorkerCliModel(vendor, cliModel);
      return next;
    });
  };

  const openWorkerLogin = async (vendor: WorkerVendor) => {
    setLoginBusy(vendor);
    setAuthError(null);
    try {
      const r = await commands.openCliLogin(vendor);
      if (r.status === "error") {
        setAuthError(r.error);
        return;
      }
      await refreshCliAuth();
    } catch (e) {
      setAuthError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoginBusy(null);
    }
  };

  const browseCwd = async () => {
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: cwd || undefined,
      });
      if (typeof picked === "string") setCwd(picked);
    } catch (e) {
      setError(
        `folder picker failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  };

  const objectiveTrimmed = objective.trim();
  const cwdTrimmed = cwd.trim();
  const ready =
    canStart &&
    !submitting &&
    objectiveTrimmed.length > 0 &&
    cwdTrimmed.length > 0;

  // List the still-empty required fields so the disabled CTA can
  // tell the user *why* it's disabled. Order matches the form: the
  // objective textarea is first, the folder picker second.
  const missingFields: string[] = [];
  if (objectiveTrimmed.length === 0) missingFields.push("objective");
  if (cwdTrimmed.length === 0) missingFields.push("folder");
  const disabledReason =
    canStart && !submitting && missingFields.length > 0
      ? missingFields.length === 2
        ? "Describe the work and choose a project folder to enable."
        : missingFields[0] === "objective"
          ? "Describe the work to enable."
          : "Choose a project folder to enable."
      : null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!ready) return;
    const { paths: scopePaths, errors: scopeErrs } =
      parseScopePaths(scopePathsText);
    if (scopeErrs.length > 0) {
      // Block submit and surface every malformed line at once — fixing
      // one and re-clicking would be tedious.
      setScopePathErrors(scopeErrs);
      setError(null);
      return;
    }
    setScopePathErrors([]);
    setSubmitting(true);
    setError(null);
    try {
      const title = deriveTitle(objectiveTrimmed);
      const encodedWorkerModel = encodeWorkerModelRoster(
        workerCount,
        workerModels,
        workerCliModels,
      );
      const result = await commands.startMission(
        {
          title,
          objective: objectiveTrimmed,
          // Empty target_ref asks the host service to resolve the
          // selected repo's current local branch. The project picker can
          // point at repos that still use `master` or another branch name.
          target_ref: "",
          tests: null,
          supervisor_model: supervisorModel,
          worker_model: encodedWorkerModel,
          worker_count:
            workerCount === "auto" ? null : Number.parseInt(workerCount, 10),
          // QC-3: 'review' maps to confirm_plan=true; 'direct' maps
          // to null (the autonomous default). The envelope-fit gate
          // can still force a pause in Direct mode if any of the
          // supervisor's four bounds reports Exceeds.
          confirm_plan: planMode === "review" ? true : null,
          // Empty array = "no constraint" on the Rust side
          // (`#[serde(default)]` on `Vec<PathBuf>`). Omitting the key
          // entirely is equivalent and keeps the IPC payload minimal
          // for the common (no scope override) case.
          ...(scopePaths.length > 0 ? { scope_paths: scopePaths } : {}),
        },
        cwdTrimmed,
      );
      if (result.status === "error") {
        setError(result.error);
        return;
      }
      // A2 (Tier-2G): record the cwd so memory IPC commands can
      // resolve the per-repo kernel. Persists across mission
      // lifecycle so the user can pin notes after accept.
      setCurrentRepoCwd(cwdTrimmed);
      setObjective("");
    } catch (err) {
      // A Tauri command can reject the promise (Rust panic, IPC
      // failure, serialization mismatch) rather than returning
      // {status:"error"}. Without this catch, `setSubmitting(false)`
      // never runs and the form is bricked until reload.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section className="deploy-panel" aria-label="Deploy workers">
      <div className="deploy-title">DEPLOY WORKERS</div>
      <form className="deploy-form" onSubmit={handleSubmit}>
        <label className="deploy-field" htmlFor="deploy-objective">
          <span className="deploy-field-label">What should the team do?</span>
          <textarea
            id="deploy-objective"
            name="objective"
            className="deploy-textarea"
            value={objective}
            onChange={(e) => setObjective(e.target.value)}
            rows={4}
            placeholder="Describe the work in plain language."
            aria-label="mission objective"
            disabled={submitting}
          />
        </label>

        <div className="deploy-field">
          <span className="deploy-field-label">Project folder</span>
          <div className="deploy-cwd-row">
            <button
              type="button"
              className="deploy-browse-btn"
              onClick={browseCwd}
              disabled={submitting}
              aria-label="browse for project folder"
            >
              {cwd ? "Change…" : "Choose folder…"}
            </button>
            <span
              className={
                cwd
                  ? "deploy-cwd-display"
                  : "deploy-cwd-display deploy-cwd-display--empty"
              }
              title={cwd || "No folder selected"}
            >
              {cwd || "No folder selected"}
            </span>
          </div>
        </div>

        <details className="deploy-advanced">
          <summary className="deploy-advanced-summary">Advanced</summary>
          <div className="deploy-advanced-body">
            <label
              className="deploy-field deploy-field--advanced"
              htmlFor="deploy-supervisor"
            >
              <span className="deploy-field-label">Supervisor</span>
              <select
                id="deploy-supervisor"
                name="supervisor"
                className="deploy-select"
                value={supervisorModel}
                disabled
                aria-label="supervisor model"
              >
                {SUPERVISOR_OPTIONS.map((m) => (
                  <option key={m} value={m}>
                    {MODEL_LABEL[m]}
                  </option>
                ))}
              </select>
              <span className="mission-review__faint">
                Additional supervisor providers are roadmap work.
              </span>
            </label>
            <label
              className="deploy-field deploy-field--advanced"
              htmlFor="deploy-worker-count"
            >
              <span className="deploy-field-label">Number of workers</span>
              <select
                id="deploy-worker-count"
                name="worker-count"
                className="deploy-select"
                value={workerCount}
                onChange={(e) =>
                  setWorkerCount(e.target.value as WorkerCountChoice)
                }
                disabled={submitting}
                aria-label="number of workers"
              >
                {WORKER_COUNT_OPTIONS.map((n) => (
                  <option key={n} value={n}>
                    {COUNT_LABEL[n]}
                  </option>
                ))}
              </select>
            </label>
            <section
              className="deploy-field deploy-field--advanced"
              aria-label="worker CLI roster"
            >
              <div className="deploy-worker-roster-head">
                <span className="deploy-field-label">Worker CLIs</span>
                <button
                  type="button"
                  className="deploy-inline-btn"
                  onClick={refreshCliAuth}
                  disabled={authBusy || submitting}
                >
                  {authBusy ? "Checking…" : "Refresh status"}
                </button>
              </div>
              {selectedWorkerCount > 0 ? (
                <div className="deploy-worker-roster">
                  {Array.from({ length: selectedWorkerCount }, (_, index) => {
                    const model =
                      workerModels[index] ??
                      DEFAULT_WORKER_MODELS[index % DEFAULT_WORKER_MODELS.length];
                    const cliModel = normalizeWorkerCliModel(
                      model,
                      workerCliModels[index],
                    );
                    const auth = authByVendor.get(model) ?? null;
                    return (
                      <WorkerCliRow
                        key={index}
                        index={index}
                        model={model}
                        cliModel={cliModel}
                        auth={auth}
                        disabled={submitting}
                        loginBusy={loginBusy === model}
                        onModelChange={(next) => setWorkerModelAt(index, next)}
                        onCliModelChange={(next) =>
                          setWorkerCliModelAt(index, next)
                        }
                        onLogin={() => openWorkerLogin(model)}
                      />
                    );
                  })}
                </div>
              ) : (
                <div className="deploy-worker-roster-empty">
                  Auto selected — the supervisor will choose worker count,
                  vendors, and applied models from the task plan.
                </div>
              )}
              {authError ? (
                <div className="deploy-auth-error" role="status">
                  CLI status unavailable: {authError}
                </div>
              ) : null}
            </section>
            <fieldset
              className="deploy-field deploy-field--advanced deploy-field--radio"
              disabled={submitting}
            >
              <legend className="deploy-field-label">Plan mode</legend>
              <div className="deploy-radio-options">
                <label
                  className="deploy-radio"
                  htmlFor="deploy-plan-mode-direct"
                >
                  <input
                    id="deploy-plan-mode-direct"
                    type="radio"
                    name="plan-mode"
                    value="direct"
                    checked={planMode === "direct"}
                    onChange={() => setPlanMode("direct" as PlanMode)}
                    aria-label="direct (auto-proceed)"
                  />
                  <span className="deploy-radio-label">Direct</span>
                </label>
                <label
                  className="deploy-radio"
                  htmlFor="deploy-plan-mode-review"
                >
                  <input
                    id="deploy-plan-mode-review"
                    type="radio"
                    name="plan-mode"
                    value="review"
                    checked={planMode === "review"}
                    onChange={() => setPlanMode("review" as PlanMode)}
                    aria-label="review (pause for plan approval)"
                  />
                  <span className="deploy-radio-label">Review</span>
                </label>
              </div>
            </fieldset>
            <label
              className="deploy-field deploy-field--advanced"
              htmlFor="deploy-scope-paths"
            >
              <span className="deploy-field-label">
                Scope paths (optional, one per line)
              </span>
              <textarea
                id="deploy-scope-paths"
                name="scope-paths"
                className="deploy-textarea deploy-textarea--scope"
                value={scopePathsText}
                onChange={(e) => {
                  setScopePathsText(e.target.value);
                  if (scopePathErrors.length > 0) setScopePathErrors([]);
                }}
                rows={3}
                placeholder={"src/\ntests/integration/"}
                aria-label="scope paths"
                spellCheck={false}
                disabled={submitting}
              />
            </label>
            {scopePathErrors.length > 0 && (
              <ul className="deploy-field-errors" role="alert">
                {scopePathErrors.map((msg) => (
                  <li key={msg}>{msg}</li>
                ))}
              </ul>
            )}
          </div>
        </details>

        {!canStart && !submitting && (
          <div className="deploy-status" role="status">
            A mission is already running. Finish or abort it first.
          </div>
        )}

        {error && (
          <div className="deploy-status deploy-status-error" role="status">
            {error}
          </div>
        )}

        <button
          type="submit"
          className={"deploy-cta" + (ready ? " deploy-cta-ready" : "")}
          disabled={!ready}
          aria-disabled={!ready}
          aria-describedby={disabledReason ? "deploy-cta-reason" : undefined}
        >
          {submitting ? "Starting…" : "Start mission"}
        </button>
        {disabledReason && (
          <div
            id="deploy-cta-reason"
            className="deploy-status deploy-status--hint"
            role="status"
            data-testid="deploy-cta-disabled-reason"
          >
            {disabledReason}
          </div>
        )}
      </form>
    </section>
  );
}

function deriveTitle(objective: string): string {
  const firstLine = objective.split("\n").find((l) => l.trim().length > 0);
  if (!firstLine) return "Mission";
  return firstLine.trim().slice(0, 60);
}
