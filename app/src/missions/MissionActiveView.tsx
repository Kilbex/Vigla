// MSV Step 6 — running-mission view.
//
// Shown inside MissionOverlay when an active mission exists and is
// not in CompletePendingMerge (Step 7 handles the review screen) or
// a terminal state. Surface is intentionally minimal per the
// "calm, not busy" principle: title + status line + progress bar +
// task list. Per-worker drill-in is hinted at but not the default.

import type { MissionSpec } from "../bindings";
import QuotaCountdown from "./QuotaCountdown";
import SupervisorStrip from "./SupervisorStrip";
import { useMissionsStore } from "./store";
import {
  selectActiveMission,
  selectAttentionItems,
  selectMissionProgress,
  selectMissionStatusLine,
  selectMissionTasks,
} from "./store";
import type { AttentionItem, MissionTask, TaskStatus } from "./types";

interface Props {
  onAbort: () => void;
  /** Elapsed-time text, e.g. "5m 14s". Receives undefined until first tick. */
  elapsed?: string;
  /** Surface the most recent abort failure so the user knows the click did not succeed. */
  abortError?: string | null;
}

export default function MissionActiveView({ onAbort, elapsed, abortError }: Props) {
  const mission = useMissionsStore(selectActiveMission);
  const statusLine = useMissionsStore(selectMissionStatusLine);
  const progress = useMissionsStore(selectMissionProgress);
  const tasks = useMissionsStore(selectMissionTasks);
  const attention = useMissionsStore(selectAttentionItems);

  if (!mission) return null;

  return (
    <div className="mission-active">
      <header className="mission-active__header">
        <h2 className="mission-active__title">{mission.spec.title}</h2>
        {elapsed && <span className="mission-active__elapsed">{elapsed}</span>}
      </header>

      {mission.spec.objective && (
        <p className="mission-active__objective">{mission.spec.objective}</p>
      )}

      <p className="mission-active__team">{formatTeam(mission.spec)}</p>

      <p className="mission-active__status">{statusLine ?? "Working…"}</p>

      <SupervisorStrip />

      <AttentionStrip items={attention} />

      <div className="mission-active__progress">
        <div
          className="mission-active__progress-bar"
          style={{ width: `${progress ?? 0}%` }}
          aria-hidden
        />
        <span className="mission-active__progress-label">
          {progress ?? 0}%
        </span>
      </div>

      <TaskList tasks={tasks} />

      <footer className="mission-active__footer">
        <button
          type="button"
          className="mission-form__button mission-form__button--secondary"
          onClick={onAbort}
        >
          Abort
        </button>
        {abortError && (
          <p className="mission-active__abort-error" role="alert">
            Abort failed: {abortError}
          </p>
        )}
      </footer>
    </div>
  );
}

function TaskList({ tasks }: { tasks: MissionTask[] }) {
  if (tasks.length === 0) {
    return <div className="mission-active__tasks-empty">Planning…</div>;
  }
  return (
    <ul className="mission-active__tasks">
      {tasks.map((t) => (
        <li
          key={t.index}
          className={`mission-active__task mission-active__task--${t.status}`}
        >
          <span
            className="mission-active__task-glyph"
            aria-hidden
          >
            {glyphFor(t.status)}
          </span>
          <span className="mission-active__task-title">{t.title}</span>
        </li>
      ))}
    </ul>
  );
}

function glyphFor(status: TaskStatus): string {
  switch (status) {
    case "pending":
      return "○";
    case "in_progress":
    case "under_review":
      return "◐";
    case "integrated":
      return "●";
    case "failed":
      return "×";
  }
}

/**
 * Phase 1 (decisions.md entries 5 & 6): render Attention items above
 * the task list. Visual treatment reuses existing color tokens —
 * `--accent-planning` (yellow) for soft, `--accent-failed` (red) for
 * hard — and no new animation is introduced (per the batch
 * constraints).
 */
export function AttentionStrip({ items }: { items: AttentionItem[] }) {
  if (items.length === 0) return null;
  return (
    <ul className="mission-active__attention" aria-label="attention">
      {items.map((item) => (
        <li
          key={`${item.kind}:${item.surfacedAt}:${item.summary}`}
          className={`mission-active__attention-item mission-active__attention-item--${item.severity}`}
          data-kind={item.kind}
        >
          <span
            className="mission-active__attention-glyph"
            aria-hidden
          >
            {item.severity === "hard" ? "!" : "·"}
          </span>
          <span className="mission-active__attention-text">{item.summary}</span>
          {item.kind === "mission_paused" && item.resumeAtMs != null && (
            <QuotaCountdown resumeAtMs={item.resumeAtMs} />
          )}
        </li>
      ))}
    </ul>
  );
}

function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function formatCliModel(model: string): string {
  if (/^(gpt|gemini)-/i.test(model)) return model;
  return model.split("-").map(capitalize).join("-");
}

function formatWorkerSelection(selection: string): string {
  const [vendorRaw, modelRaw] = selection.split(":", 2);
  const vendor = capitalize(vendorRaw.trim());
  const model = modelRaw?.trim();
  return model ? `${vendor} ${formatCliModel(model)}` : vendor;
}

function formatWorkerModel(model: string): string {
  const roster = model
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
  if (roster.length <= 1) return formatWorkerSelection(model);
  return roster.map(formatWorkerSelection).join(" / ");
}

/**
 * Build the small "Claude supervisor · N Codex workers" line shown
 * under the objective so the user can confirm at a glance that their
 * Advanced selection took effect. `worker_count` and `worker_model`
 * may both be null (auto role routing / supervisor task count) — in
 * that case we show "auto workers" rather than fake-precise numbers.
 */
export function formatTeam(spec: MissionSpec): string {
  const supervisor = capitalize(spec.supervisor_model ?? "claude");
  const count = spec.worker_count;
  const workerVendor = spec.worker_model;

  let workers: string;
  if (count !== null && workerVendor !== null) {
    workers = `${count} ${formatWorkerModel(workerVendor)} ${count === 1 ? "worker" : "workers"}`;
  } else if (count !== null) {
    workers = `${count} ${count === 1 ? "worker" : "workers"}`;
  } else if (workerVendor !== null) {
    workers = `${formatWorkerModel(workerVendor)} workers`;
  } else {
    workers = "auto workers";
  }

  return `${supervisor} supervisor · ${workers}`;
}
