import { lazy, Suspense, useEffect, useRef, useState } from "react";
import type { Event } from "../bindings";
import { commands } from "../bindings";
import {
  selectActiveMission,
  useMissionsStore,
} from "../missions/store";
import {
  selectIsLead,
  selectReviewStatus,
  selectSelectedWorkerId,
  selectSquadIds,
  selectWorker,
  selectWorkerEvents,
  useOpsStore,
} from "../store";
import EventFeed from "./EventFeed";
import FilesTab from "./FilesTab";
import PlanTab from "./PlanTab";
import Result from "./Result";
import { CostStrip, TestsStrip } from "./Strips";
import WorkerPortrait from "./WorkerPortrait";
import { useDialogFocus } from "../useDialogFocus";

// xterm + its addons + the xterm css together dominate the drawer's
// dependency cost (~hundreds of KB minified). Most workers never have
// the terminal tab opened — clicking is opt-in — so we defer the
// import until then. Mounted inside the terminal-tab branch below
// with a `null` Suspense fallback: the drawer-terminal div is already
// black, so a momentary blank is invisible vs. xterm's own painting
// pass.
const RawTerminal = lazy(() => import("./RawTerminal"));

// QC-3: "plan" tab surfaces only when the active mission has a
// proposed plan attached (PlanProposed emitted at least once). Kept
// at the end of the tab strip so the existing per-worker tab order
// isn't perturbed.
const BASE_TABS = ["result", "feed", "terminal", "files", "tests", "cost"] as const;
const MISSION_TABS = ["result", "feed", "tests", "cost"] as const;
type BaseTab = (typeof BASE_TABS)[number];
type Tab = BaseTab | "plan";

const REVIEW_ACTION_LABEL: Record<
  "needs_review" | "accepted" | "rejected" | "parked",
  string
> = {
  needs_review: "Needs review",
  accepted: "Accept",
  rejected: "Reject",
  parked: "Park",
};

const REVIEW_ACTION_TITLE: Record<keyof typeof REVIEW_ACTION_LABEL, string> = {
  needs_review: "needs review",
  accepted: "accepted",
  rejected: "rejected",
  parked: "parked",
};

/// Done/failed workers land on the result tab so the user reads the
/// final summary before scrubbing the chronological event feed; every
/// other state defaults to feed, the natural live-tail view.
function defaultTabFor(state: string): Tab {
  return state === "done" || state === "failed" ? "result" : "feed";
}

function commandErrorMessage(error: unknown): string {
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  return message.trim() || "Unexpected IPC failure. Try again.";
}

interface WorkerActionContext {
  workerId: string;
  generation: number;
}

// Stable sentinels for the no-worker-selected branch. Inline `() => []`
// allocates a new array on every getSnapshot call, defeating Zustand's
// Object.is short-circuit and forcing useSyncExternalStore into an
// infinite re-render loop (React error #185 in production). The
// terminal-true `() => undefined` and `() => false` cases below are
// safe because primitives compare equal under Object.is.
const NO_EVENTS: Event[] = [];
const selectNoEvents = () => NO_EVENTS;

export default function Drawer() {
  const drawerRef = useRef<HTMLElement | null>(null);
  useDialogFocus(true, drawerRef, false);
  const workerId = useOpsStore(selectSelectedWorkerId);
  const select = useOpsStore((s) => s.selectWorker);
  const worker = useOpsStore(workerId ? selectWorker(workerId) : () => undefined);
  const events = useOpsStore(workerId ? selectWorkerEvents(workerId) : selectNoEvents);
  const setWorkerModel = useOpsStore((s) => s.setWorkerModel);
  const squadIds = useOpsStore(selectSquadIds);
  const squads = useOpsStore((s) => s.squads);
  const workerSquadId = useOpsStore((s) => (workerId ? s.workerSquad[workerId] : undefined));
  const assignWorkerToSquad = useOpsStore((s) => s.assignWorkerToSquad);
  const isLead = useOpsStore(workerId ? selectIsLead(workerId) : () => false);
  const setReviewStatus = useOpsStore((s) => s.setReviewStatus);
  // Subscribe to the reviewStatus slice reactively. Reading it via the
  // imperative `getReviewStatus` getter only subscribed to that stable
  // function reference, so clicking Accept/Reject/Park never re-rendered
  // the buttons (stale highlight + wrong aria-pressed).
  const reviewStatus = useOpsStore(
    workerId ? selectReviewStatus(workerId) : () => undefined,
  );
  const activeMission = useMissionsStore(selectActiveMission);
  const hasPlan = !!activeMission && activeMission.tasks.length > 0;
  const missionScoped = worker?.missionScoped ?? false;
  const baseTabs: readonly BaseTab[] = missionScoped ? MISSION_TABS : BASE_TABS;
  const TABS: readonly Tab[] = hasPlan ? [...baseTabs, "plan"] : baseTabs;
  const [tab, setTab] = useState<Tab>("feed");
  const [stopBusy, setStopBusy] = useState(false);
  const [stopError, setStopError] = useState<string | null>(null);
  const [retryBusy, setRetryBusy] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [followUpPrompt, setFollowUpPrompt] = useState("");
  const [followUpBusy, setFollowUpBusy] = useState(false);
  const [followUpError, setFollowUpError] = useState<string | null>(null);
  const [modelInput, setModelInput] = useState("");
  const [modelBusy, setModelBusy] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [modelStatus, setModelStatus] = useState<string | null>(null);
  const activeWorkerIdRef = useRef(workerId);
  const workerGenerationRef = useRef(0);

  // Reset the active tab to the state-appropriate default whenever the
  // selected worker changes. Manual tab choices still win within a
  // single open session — switching workers (or re-opening) re-applies
  // the default.
  useEffect(() => {
    if (!workerId || !worker) return;
    setTab(defaultTabFor(worker.state));
    // We intentionally only depend on workerId so a state transition
    // (executing → done) on the *same* selected worker does not yank
    // the user off the tab they are currently reading.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workerId]);

  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // ESC closes the drawer — but not while the user is typing in one of the
  // drawer's text controls (follow-up textarea, model input, feed search).
  // keyboard.ts's global handler deliberately defers Escape to the local
  // component (cascade §4.3, step 1 = text input); without this focus guard
  // the first Escape would unmount the drawer and silently discard the
  // in-progress follow-up draft.
  useEffect(() => {
    if (!workerId || missionScoped) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName?.toLowerCase();
      if (
        tag === "input" ||
        tag === "textarea" ||
        tag === "select" ||
        target?.isContentEditable
      ) {
        return;
      }
      select(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [workerId, missionScoped, select]);

  // Action state belongs to one drawer selection. Reset it when the
  // worker changes; the generation token keeps a late completion
  // from the previous worker from clearing or erroring the new one.
  useEffect(() => {
    if (activeWorkerIdRef.current !== workerId) {
      workerGenerationRef.current += 1;
    }
    activeWorkerIdRef.current = workerId;
    setStopBusy(false);
    setStopError(null);
    setRetryBusy(false);
    setRetryError(null);
    setFollowUpPrompt("");
    setFollowUpBusy(false);
    setFollowUpError(null);
    setModelBusy(false);
    setModelError(null);
    setModelStatus(null);
    setModelInput(worker?.model ?? "");
  }, [workerId]);

  useEffect(() => {
    if (!workerId) return;
    let cancelled = false;
    commands
      .getWorkerInfo(workerId)
      .then((r) => {
        if (cancelled || r.status === "error") return;
        // Only overwrite when the row actually carries a model. A
        // WorkerInfo row can exist with model=null (the cost event that
        // fills it in hasn't persisted yet) — writing that null would
        // clobber a model the live cost-event stream already set in the
        // store, blanking the drawer and the worker tile.
        if (r.data.model) {
          setWorkerModel(workerId, r.data.model);
          setModelInput(r.data.model);
        }
      })
      .catch(() => {
        // Mission-scoped workers may not have standalone WorkerInfo
        // rows yet. The live event stream still updates model from
        // cost events, so this lookup is best-effort.
      });
    return () => {
      cancelled = true;
    };
  }, [workerId, missionScoped, setWorkerModel]);

  if (!workerId || !worker) return null;

  const captureWorkerContext = (): WorkerActionContext => ({
    workerId,
    generation: workerGenerationRef.current,
  });
  const isCurrentWorkerContext = (context: WorkerActionContext): boolean =>
    mountedRef.current &&
    activeWorkerIdRef.current === context.workerId &&
    workerGenerationRef.current === context.generation &&
    useOpsStore.getState().selectedWorkerId === context.workerId;

  const stop = async () => {
    const context = captureWorkerContext();
    setStopBusy(true);
    setStopError(null);
    try {
      const r = await commands.stopWorker(workerId);
      if (!isCurrentWorkerContext(context)) return;
      if (r.status === "error") setStopError(r.error);
    } catch (error) {
      if (isCurrentWorkerContext(context)) {
        setStopError(commandErrorMessage(error));
      }
    } finally {
      if (isCurrentWorkerContext(context)) setStopBusy(false);
    }
  };

  const retry = async () => {
    const context = captureWorkerContext();
    setRetryBusy(true);
    setRetryError(null);
    try {
      const r = await commands.retryWorker(workerId);
      if (!isCurrentWorkerContext(context)) return;
      if (r.status === "error") setRetryError(r.error);
    } catch (error) {
      if (isCurrentWorkerContext(context)) {
        setRetryError(commandErrorMessage(error));
      }
    } finally {
      if (isCurrentWorkerContext(context)) setRetryBusy(false);
    }
  };

  const sendFollowUp = async () => {
    if (!followUpPrompt.trim()) return;
    const context = captureWorkerContext();
    const prompt = followUpPrompt;
    setFollowUpBusy(true);
    setFollowUpError(null);
    try {
      const r = await commands.continueWorker(workerId, prompt);
      if (!isCurrentWorkerContext(context)) return;
      if (r.status === "error") {
        setFollowUpError(r.error);
      } else {
        setFollowUpPrompt("");
      }
    } catch (error) {
      if (isCurrentWorkerContext(context)) {
        setFollowUpError(commandErrorMessage(error));
      }
    } finally {
      if (isCurrentWorkerContext(context)) setFollowUpBusy(false);
    }
  };

  const switchModel = async () => {
    const nextModel = modelInput.trim();
    if (!nextModel) return;
    const context = captureWorkerContext();
    setModelBusy(true);
    setModelError(null);
    setModelStatus(null);
    try {
      const r = await commands.switchWorkerModel(workerId, nextModel);
      if (!isCurrentWorkerContext(context)) return;
      if (r.status === "error") {
        setModelError(r.error);
        return;
      }
      setWorkerModel(workerId, r.data.model);
      setModelStatus(r.data.detail);
    } catch (error) {
      if (isCurrentWorkerContext(context)) {
        setModelError(commandErrorMessage(error));
      }
    } finally {
      if (isCurrentWorkerContext(context)) setModelBusy(false);
    }
  };

  const canRetry =
    !missionScoped && (worker.state === "done" || worker.state === "failed");
  const canSendFollowUp =
    !missionScoped &&
    worker.state !== "executing" &&
    worker.state !== "planning" &&
    worker.state !== "reviewing" &&
    worker.state !== "blocked";

  const stillRunning =
    !missionScoped && worker.state !== "done" && worker.state !== "failed";

  return (
    <>
      <button
        className="drawer-scrim"
        onClick={() => select(null)}
        aria-label="close drawer"
      />
      <section
        ref={drawerRef}
        className="drawer"
        role="dialog"
        aria-modal="false"
        aria-label={`Worker ${worker.shortId}`}
      >
        <header className="drawer-head">
          <div className="drawer-head-row">
            <WorkerPortrait vendor={worker.vendor} state={worker.state} />
            <div className="drawer-title">
              <span className="drawer-callsign">{worker.shortId}</span>
              <span className={`drawer-state drawer-state--${worker.state}`}>
                {worker.state}
              </span>
              {isLead ? (
                <span className="drawer-lead-tag" aria-label="squad lead">
                  ▲ lead
                </span>
              ) : null}
              {worker.currentTaskTitle ? (
                <span className="drawer-task">{worker.currentTaskTitle}</span>
              ) : null}
            </div>
            <div className="drawer-actions">
              {stillRunning ? (
                <button
                  className="drawer-btn drawer-btn-stop"
                  onClick={stop}
                  disabled={stopBusy}
                >
                  {stopBusy ? "stopping…" : "stop"}
                </button>
              ) : null}
              {canRetry ? (
                <button
                  className="drawer-btn drawer-btn-retry"
                  onClick={retry}
                  disabled={retryBusy}
                >
                  {retryBusy ? "retrying…" : "retry"}
                </button>
              ) : null}
              <button
                className="drawer-btn"
                onClick={() => select(null)}
                aria-label="close"
              >
                close (esc)
              </button>
            </div>
          </div>
          {stopError ? (
            <div
              className="comms-status comms-status-error"
              role="alert"
            >
              stop failed: {stopError}
            </div>
          ) : null}
          {retryError ? (
            <div
              className="comms-status comms-status-error"
              role="alert"
            >
              retry failed: {retryError}
            </div>
          ) : null}
          <div className="drawer-squad" role="group" aria-label="squad assignment">
            <label className="drawer-squad-label" htmlFor="drawer-squad-select">
              squad
            </label>
            <select
              id="drawer-squad-select"
              className="drawer-squad-select"
              value={workerSquadId ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                assignWorkerToSquad(workerId, v === "" ? null : v);
              }}
            >
              <option value="">unassigned</option>
              {squadIds.map((sid) => {
                const sq = squads[sid];
                if (!sq) return null;
                return (
                  <option key={sid} value={sid}>
                    {sq.name}
                  </option>
                );
              })}
            </select>
          </div>
          {!missionScoped ? (
          <div className="drawer-model" role="group" aria-label="model for next continuation">
            <span className="drawer-model-label">model</span>
            <span
              className={
                "drawer-model-current" +
                (worker.model ? "" : " drawer-model-current--unknown")
              }
              title={worker.model ?? "No model observed yet"}
            >
              {worker.model ?? "default"}
            </span>
            <input
              className="drawer-model-input"
              value={modelInput}
              onChange={(e) => setModelInput(e.target.value)}
              placeholder="model name"
              disabled={modelBusy}
              aria-label="model name"
            />
            <button
              type="button"
              className="drawer-btn drawer-model-btn"
              onClick={switchModel}
              disabled={!modelInput.trim() || modelBusy}
            >
              {modelBusy ? "saving…" : "use next"}
            </button>
          </div>
          ) : (
            <div className="comms-status" role="status">
              Mission worker controls are managed from the mission surface.
            </div>
          )}
          {modelError ? (
            <div className="comms-status comms-status-error" role="alert">
              model switch failed: {modelError}
            </div>
          ) : null}
          {modelStatus ? (
            <div className="comms-status" role="status">
              {modelStatus}
            </div>
          ) : null}
          {canRetry && (
            <div className="drawer-review-actions" role="group" aria-label="review status">
              {(["needs_review", "accepted", "rejected", "parked"] as const).map((status) => (
                <button
                  key={status}
                  className={[
                    "review-status-btn",
                    `review-status-btn--${status}`,
                    reviewStatus === status ? "review-status-btn--active" : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={() => setReviewStatus(workerId!, status)}
                  title={`Mark as ${REVIEW_ACTION_TITLE[status]}`}
                  aria-pressed={reviewStatus === status}
                >
                  {REVIEW_ACTION_LABEL[status]}
                </button>
              ))}
            </div>
          )}
        </header>
        <nav className="drawer-tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t}
              role="tab"
              aria-selected={tab === t}
              className={"drawer-tab" + (tab === t ? " drawer-tab--on" : "")}
              onClick={() => setTab(t)}
            >
              {t}
            </button>
          ))}
        </nav>
        <div className="drawer-body">
          {tab === "result" && <Result worker={worker} events={events} />}
          {tab === "feed" &&
            (missionScoped ? (
              <ol className="drawer-mission-timeline" aria-label="Mission worker timeline">
                {worker.missionTimeline.map((entry, index) => (
                  <li key={`${entry.ts}:${index}`}>
                    <strong>{entry.label}</strong>
                    {entry.detail ? <span>{entry.detail}</span> : null}
                    <time dateTime={entry.ts}>{entry.ts}</time>
                  </li>
                ))}
              </ol>
            ) : (
              <EventFeed events={events} />
            ))}
          {tab === "terminal" && (
            <Suspense fallback={null}>
              <RawTerminal events={events} workerId={workerId} />
            </Suspense>
          )}
          {tab === "files" && <FilesTab events={events} workerId={workerId} />}
          {tab === "tests" && <TestsStrip events={events} />}
          {tab === "cost" && <CostStrip events={events} />}
          {tab === "plan" && activeMission ? (
            <PlanTab mission={activeMission} />
          ) : null}
          {!missionScoped && (tab === "result" || tab === "feed") && (
            <div className="drawer-followup">
              {followUpError ? (
                <div
                  className="comms-status comms-status-error"
                  role="alert"
                >
                  follow-up failed: {followUpError}
                </div>
              ) : null}
              <textarea
                className="drawer-followup-input"
                placeholder={canSendFollowUp ? "Send a follow-up prompt…" : "Worker is busy"}
                value={followUpPrompt}
                onChange={(e) => setFollowUpPrompt(e.target.value)}
                disabled={!canSendFollowUp || followUpBusy}
                aria-label="Follow-up prompt"
              />
              <button
                className="drawer-followup-btn"
                onClick={sendFollowUp}
                disabled={!canSendFollowUp || !followUpPrompt.trim() || followUpBusy}
                aria-label="Send follow-up"
              >
                {followUpBusy ? "sending…" : "send"}
              </button>
            </div>
          )}
        </div>
      </section>
    </>
  );
}
