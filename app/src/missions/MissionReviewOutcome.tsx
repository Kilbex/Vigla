// MSV Step 7 — Review Outcome screen.
//
// Appears in the overlay when lifecycle === complete_pending_merge.
// Shows the mission summary, file list aggregated from each worker's
// submission, test status, elapsed time, and the two executable
// terminal actions: Merge / Discard. Post-verdict continuation stays
// hidden until the runtime can schedule a real supervisor re-entry.

import { useMemo, useState } from "react";
import { commands } from "../bindings";
import { formatTeam } from "./MissionActiveView";
import { formatTestsForMission, isTestsRowFallback } from "./trustSnapshot";
import type { ActiveMission } from "./types";

interface Props {
  mission: ActiveMission;
  elapsed?: string;
  onResolved?: () => void;
}

export { TESTS_ROW_NO_DATA, formatTestsForMission as formatTestsRow, isTestsRowFallback } from "./trustSnapshot";

export default function MissionReviewOutcome({
  mission,
  elapsed,
  onResolved,
}: Props) {
  const [submitting, setSubmitting] = useState<"merge" | "discard" | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Aggregate file names from every worker's submission. Sort+dedup
  // so the same file submitted twice (e.g. revised) shows once.
  const files = useMemo(() => {
    const set = new Set<string>();
    for (const w of Object.values(mission.workers)) {
      for (const f of w.submittedFiles) set.add(f);
    }
    return Array.from(set).sort();
  }, [mission.workers]);

  const runResolve = async (kind: "merge" | "discard") => {
    setSubmitting(kind);
    setError(null);
    try {
      const result = await commands.resolveMission({ type: kind });
      if (result.status === "error") {
        if (isAlreadyTerminalResolveError(result.error)) {
          onResolved?.();
          return;
        }
        setError(result.error);
        return;
      }
      onResolved?.();
    } catch (caught) {
      // The binding re-throws on an Error-instance rejection (IPC failure,
      // serialization mismatch). Without this catch, setSubmitting(null)
      // in `finally` never runs and both Merge/Discard buttons stay
      // permanently disabled with no error surfaced. Mirrors the guard on
      // MissionOverlay.handleAbort and PlanRejectForm.submit.
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setSubmitting(null);
    }
  };

  const disabled = submitting !== null;

  // Count only tasks that actually integrated. This surface is also
  // reached via the arbiter-escalation → attention path, where a task
  // can be `failed`/`pending`, so the total task count would overstate
  // how much landed. Mirrors the reducer's `Integrated n/total` line.
  const integratedCount = mission.tasks.filter(
    (t) => t.status === "integrated",
  ).length;

  // Phase 1 (decisions.md entry 7): when the mission is paused at
  // `attention` (sub-supervisor-refused), the overlay surfaces this
  // same review surface so the user can resolve via
  // Merge / Discard. Adapt the title glyph and default
  // summary to make clear the mission is paused — not completed —
  // and surface the active Attention items so the user has context
  // for the decision they're being asked to make.
  const isPaused = mission.lifecycle === "attention";
  const titleGlyph = isPaused ? "!" : "✓";
  const checkClass = isPaused
    ? "mission-review__check mission-review__check--paused"
    : "mission-review__check";
  const defaultSummary = isPaused
    ? "Mission paused. Review the Attention items below and decide how to proceed."
    : "Mission complete.";

  return (
    <div className="mission-review">
      <header className="mission-review__header">
        <h2 className="mission-review__title">
          <span className={checkClass} aria-hidden>
            {titleGlyph}
          </span>
          {mission.spec.title}
        </h2>
        {elapsed && <span className="mission-active__elapsed">{elapsed}</span>}
      </header>

      <p className="mission-review__summary">
        {mission.completionSummary ?? defaultSummary}
      </p>

      {isPaused && mission.attention.length > 0 && (
        <ul className="mission-active__attention" aria-label="attention">
          {mission.attention.map((item) => (
            <li
              key={`${item.kind}:${item.surfacedAt}:${item.summary}`}
              className={`mission-active__attention-item mission-active__attention-item--${item.severity}`}
              data-kind={item.kind}
            >
              <span className="mission-active__attention-glyph" aria-hidden>
                {item.severity === "hard" ? "!" : "·"}
              </span>
              <span className="mission-active__attention-text">
                {item.summary}
              </span>
            </li>
          ))}
        </ul>
      )}

      <p className="mission-active__team">{formatTeam(mission.spec)}</p>

      <section className="mission-review__section">
        <h3 className="mission-review__section-title">Files changed</h3>
        {files.length === 0 ? (
          // Fall back to mission.completed's `files_changed` count when
          // per-worker submissions are absent (e.g. submitted events lost).
          mission.filesChanged > 0 ? (
            <p className="mission-review__faint">
              {mission.filesChanged} files changed (details unavailable)
            </p>
          ) : (
            <p className="mission-review__faint">No files reported.</p>
          )
        ) : (
          <ul className="mission-review__files">
            {files.map((f) => (
              <li key={f} className="mission-review__file">
                {f}
              </li>
            ))}
          </ul>
        )}
      </section>

      <dl className="mission-review__meta">
        <div className="mission-review__meta-row">
          <dt>Tests</dt>
          <dd>
            {(() => {
              const value = formatTestsForMission(mission);
              const isNoData = isTestsRowFallback(value);
              return (
                <span className={isNoData ? "mission-review__faint" : ""}>
                  {value}
                </span>
              );
            })()}
          </dd>
        </div>
        <div className="mission-review__meta-row">
          <dt>Tasks</dt>
          <dd>
            {integratedCount}/{mission.tasks.length} integrated
          </dd>
        </div>
        {elapsed && (
          <div className="mission-review__meta-row">
            <dt>Time</dt>
            <dd>{elapsed}</dd>
          </div>
        )}
      </dl>

      {error && <div className="mission-form__error">{error}</div>}

      {integratedCount === 0 && (
        <p className="mission-review__faint">
          Nothing was integrated; discard this mission instead.
        </p>
      )}

      <div className="mission-review__actions">
        <button
          type="button"
          className="mission-form__button mission-form__button--tertiary"
          onClick={() => runResolve("discard")}
          disabled={disabled}
        >
          {submitting === "discard" ? "Discarding…" : "Discard"}
        </button>
        <button
          type="button"
          className="mission-form__button mission-form__button--primary"
          onClick={() => runResolve("merge")}
          disabled={disabled || integratedCount === 0}
        >
          {submitting === "merge" ? "Merging…" : "Merge"}
        </button>
      </div>
    </div>
  );
}

function isAlreadyTerminalResolveError(message: string): boolean {
  return /resolve not allowed from state\s+(merged|discarded)\b/i.test(
    message,
  );
}
