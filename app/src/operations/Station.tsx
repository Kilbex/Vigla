import { Handle, Position, type NodeProps } from "@xyflow/react";
import type { Node } from "@xyflow/react";
import { useEffect, useState } from "react";
import { selectIsLead, selectSquadOf, useOpsStore } from "../store";
import type { WorkerSnapshot } from "../store/types";
import { getVendorGlyph } from "./avatar";
import WorkerAvatar from "./WorkerAvatar";
import { useWorkerIdentity } from "./useWorkerIdentity";

const STATE_LABEL: Record<string, string> = {
  idle: "AWAITING ORDERS",
  planning: "PLANNING",
  executing: "EXECUTING",
  blocked: "BLOCKED",
  reviewing: "REVIEWING",
  done: "DONE",
  failed: "FAILED",
};

function formatPercent(p: number | null): string {
  if (p === null) return "—";
  return p >= 100 ? "100%" : `${Math.round(p)}%`;
}

function formatEta(ms: number | null): string | null {
  if (ms === null || ms <= 0) return null;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

// React Flow's `Node` generic requires `data extends Record<string,
// unknown>`. The injected snapshot is the entire WorkerSnapshot.
export type StationData = WorkerSnapshot & Record<string, unknown>;
export type StationNode = Node<StationData, "station">;

/// Worker station tile. Pure projection of a [`WorkerSnapshot`]; the
/// store handles the reduction.
export default function Station({ data }: NodeProps<StationNode>) {
  const select = useOpsStore((s) => s.selectWorker);
  const squad = useOpsStore(selectSquadOf(data.id));
  const isLead = useOpsStore(selectIsLead(data.id));
  const getReviewStatus = useOpsStore((s) => s.getReviewStatus);
  const reviewStatus = getReviewStatus(data.id);
  const identity = useWorkerIdentity(data.id);
  const [now, setNow] = useState<number>(() => Date.now());

  // Identity overlay (P1-6): prefer authoritative WorkerInfo from the
  // orchestrator when available, falling back to the snapshot defaults
  // that fresh() seeds with vendor:"mock", model:null.
  const effectiveVendor = identity?.vendor ?? data.vendor;
  const effectiveModel = identity?.model ?? data.model;
  const vendorGlyph = getVendorGlyph(effectiveVendor);
  const vendorLabel = vendorGlyph.vendorLabel;
  // Hide the chip for the placeholder vendors so the unmistakable
  // ones (claude / codex / gemini / opencode) read as identity rather
  // than noise. `getVendorGlyph` collapses any non-Vendor string to
  // `"unknown"`, so checking the hue covers forward-compat too.
  const showVendorChip =
    vendorGlyph.hue !== "mock" && vendorGlyph.hue !== "unknown";

  // The "done" flash fades over ~1.8s. Only run a ticker while the
  // flash window is still open — otherwise an idle, never-finishing
  // station would keep re-rendering at 4 Hz forever (and N stations
  // mean N timers, defeating the 60 fps target).
  const flashUntil = data.flashUntil;
  useEffect(() => {
    if (flashUntil <= Date.now()) return;
    const id = window.setInterval(() => {
      const t = Date.now();
      setNow(t);
      if (t >= flashUntil) {
        window.clearInterval(id);
      }
    }, 250);
    return () => window.clearInterval(id);
  }, [flashUntil]);

  const flashing = flashUntil > now;
  const executingScan = data.state === "executing" ? " hud-scanline" : "";
  const stateClass =
    `station station--${data.state}${executingScan}` +
    (flashing ? " station--flash" : "");
  const idleBreath = data.state === "idle" ? "station--breath" : "";
  const blockedHint = data.blockedOn?.length
    ? `blocked on ${data.blockedOn.length === 1 ? data.blockedOn[0].slice(-8) : `${data.blockedOn.length} tasks`}`
    : null;
  const stateNote =
    data.state === "blocked"
      ? blockedHint
      : data.state === "failed"
        ? data.failureSummary
        : data.state === "done"
          ? data.completionSummary?.split(/[.;]/)[0] ?? null
          : data.progressNote;

  const handleSelect = () => {
    select(data.id);
  };

  return (
    <div
      className={`${stateClass} ${idleBreath}`}
      role="button"
      tabIndex={0}
      onClick={handleSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          handleSelect();
        }
      }}
    >
      {/* React Flow expects handles on a custom node so edges connect. */}
      <Handle
        type="target"
        position={Position.Left}
        className="station__handle station__handle--in"
      />
      <Handle
        type="source"
        position={Position.Right}
        className="station__handle station__handle--out"
      />

      {/*
        Step 19 — squad-color bar at the top of the tile. Always
        rendered so vendor/state changes never reflow this region;
        background is transparent when the worker is unassigned.
      */}
      <div
        className={
          "station__squad-bar" +
          (squad ? ` station__squad-bar--${squad.color}` : "")
        }
        title={squad ? `Squad: ${squad.name}` : undefined}
        aria-hidden={!squad}
        aria-label={squad ? `Squad ${squad.name}` : undefined}
        data-squad-id={squad?.id ?? ""}
      />

      {/*
        Step 21 — squad lead chevron. Renders at the top-left of the
        tile when this worker is its squad's designated lead. Tinted
        by the squad's color so the badge reads as "lead of {squad}."
      */}
      {isLead && squad ? (
        <span
          className={`station__lead-badge station__lead-badge--${squad.color}`}
          title={`Squad lead — ${squad.name}`}
          aria-label={`Squad lead of ${squad.name}`}
          data-testid="station-lead-badge"
        >
          ▲
        </span>
      ) : null}

      {/* Batch 2 — review status badge for done/failed workers */}
      {reviewStatus && (data.state === "done" || data.state === "failed") ? (
        <span
          className={`station__review-badge station__review-badge--${reviewStatus}`}
          title={`Review status: ${reviewStatus}`}
          aria-label={`Review status: ${reviewStatus}`}
          data-testid="station-review-badge"
        >
          {reviewStatus === "accepted" ? "✓" : reviewStatus === "rejected" ? "✗" : reviewStatus === "parked" ? "⊘" : "○"}
        </span>
      ) : null}

      <header className="station__head">
        <WorkerAvatar vendor={effectiveVendor} state={data.state} />
        {(data.state === "executing" || data.state === "planning") && (
          <span className="station__live-dot" aria-hidden />
        )}
        <span
          className="station__callsign"
          title={data.id}
        >
          {data.shortId}
        </span>
        {showVendorChip ? (
          <span className="station__vendor" title={vendorLabel}>
            {vendorLabel}
          </span>
        ) : null}
        <span className="station__state">{STATE_LABEL[data.state] ?? data.state}</span>
      </header>

      <div className="station__title" title={data.currentTaskTitle ?? ""}>
        {data.currentTaskTitle ?? data.completionSummary ?? "—"}
      </div>

      <div
        className={
          "station__model" +
          (effectiveModel ? "" : " station__model--unknown")
        }
        title={effectiveModel ?? "Awaiting first model event"}
      >
        {effectiveModel ?? "model pending"}
      </div>

      {/* Terminal-state display: a `done` worker shows 100% even if the
          script/CLI never emitted a final 100 progress event (most don't —
          they signal completion via state=done or a `completion` event).
          ETA is suppressed for `done` and `failed` since neither has a
          meaningful "time remaining". */}
      {(() => {
        const isDone = data.state === "done";
        const isFailed = data.state === "failed";
        const displayProgress = isDone ? 100 : data.progress;
        const displayEta = isDone || isFailed ? null : data.etaMs;
        return (
          <>
            <div className="station__progress" aria-hidden>
              <div
                className="station__progress-bar"
                style={{ width: `${Math.max(0, Math.min(100, displayProgress ?? 0))}%` }}
              />
            </div>
            <div className="station__progress-meta">
              <span>{formatPercent(displayProgress)}</span>
              {formatEta(displayEta) ? <span>{formatEta(displayEta)} eta</span> : null}
            </div>
          </>
        );
      })()}

      {(() => {
        // P2-18 — hide zero-only counter chips. Cost always renders
        // (it's a real signal even at $0.000); files and tests are
        // noise until the worker has actually done something.
        const showFilesCounter =
          data.filesAdded > 0 || data.filesModified > 0;
        const showTestsCounter =
          data.testsPassed > 0 || data.testsFailed > 0;
        return (
          <footer className="station__footer">
            {showFilesCounter && (
              <span
                className="station__counter"
                title="files added / modified"
              >
                {`+${data.filesAdded}/~${data.filesModified}`}
              </span>
            )}
            {showTestsCounter && (
              <span
                className={
                  "station__counter" +
                  (data.testsFailed > 0 ? " station__counter--alert" : "")
                }
                title="tests passed / failed"
              >
                {`${data.testsPassed}✓ ${data.testsFailed}✗`}
              </span>
            )}
            <span className="station__counter" title="cost (USD)">
              {`$${data.costUsd.toFixed(3)}`}
            </span>
          </footer>
        );
      })()}

      {stateNote ? (
        <div className="station__note" title={stateNote}>
          {stateNote}
        </div>
      ) : null}
      {data.state === "done" || data.state === "failed" ? (
        <div className="station__note-open" aria-hidden>
          ↗ open full output
        </div>
      ) : null}
    </div>
  );
}
