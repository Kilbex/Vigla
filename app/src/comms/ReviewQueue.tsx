import { useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { commands } from "../bindings";
import { selectWorkersNeedingReview, useOpsStore } from "../store";
import type { Vendor } from "../bindings";

/// Review-queue actionable cards.
///
/// Each card surfaces 5 inline actions — open, retry, continue,
/// accept, reject — plus an inline (non-modal) continue textarea
/// that expands underneath the action row. The card is the
/// keyboard-first triage surface; J/K/O/R/⇧R/A/X act on the
/// "focused" card whose id is tracked in
/// `OpsState.reviewFocusedWorkerId`.
///
/// Retry / continue are Claude-only because it is the sole vendor with a
/// verified resume contract. Disabled buttons stay rendered with a tooltip so
/// the capability boundary is visible rather than hidden.

const VENDOR_CAN_RETRY: Record<Vendor, boolean> = {
  claude: true,
  codex: false,
  gemini: false,
  antigravity: false,
  kiro: false,
  copilot: false,
  opencode: false,
  mock: false,
};

function relativeTime(spawnedAt: number, now: number): string {
  const delta = Math.max(0, now - spawnedAt);
  const s = Math.floor(delta / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

export default function ReviewQueue() {
  const workerIds = useOpsStore(useShallow(selectWorkersNeedingReview));
  const workers = useOpsStore((s) => s.workers);
  const selectedWorkerId = useOpsStore((s) => s.selectedWorkerId);
  const focusedWorkerId = useOpsStore((s) => s.reviewFocusedWorkerId);
  const isReplay = useOpsStore((s) => s.replay.mode === "replay");
  const selectWorker = useOpsStore((s) => s.selectWorker);
  const setReviewStatus = useOpsStore((s) => s.setReviewStatus);
  const setReviewFocus = useOpsStore((s) => s.setReviewFocus);
  const getReviewStatus = useOpsStore((s) => s.getReviewStatus);

  // Per-card inline continue state.
  const [continueExpandedFor, setContinueExpandedFor] = useState<string | null>(null);
  const [continueText, setContinueText] = useState("");
  const [continueBusy, setContinueBusy] = useState(false);
  const [continueError, setContinueError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Sort workers by spawn time (newest first) — stable, deterministic
  // ordering; the selector preserves `workerOrder` (insertion).
  const orderedIds = useMemo(() => {
    return [...workerIds].sort(
      (a, b) => (workers[b]?.spawnedAt ?? 0) - (workers[a]?.spawnedAt ?? 0),
    );
  }, [workerIds, workers]);

  // Listen for the keyboard map's request to expand the inline
  // continue area. Decoupled via window event so the keyboard
  // handler doesn't have to know about per-component state.
  useEffect(() => {
    const onExpand = (e: Event) => {
      const wid = (e as CustomEvent<{ workerId: string }>).detail?.workerId;
      if (!wid) return;
      const w = workers[wid];
      if (!w || !VENDOR_CAN_RETRY[w.vendor]) return;
      setContinueExpandedFor(wid);
      setContinueError(null);
      queueMicrotask(() => textareaRef.current?.focus());
    };
    window.addEventListener("vigla:continue-expand", onExpand);
    return () =>
      window.removeEventListener("vigla:continue-expand", onExpand);
  }, [workers]);

  // The continue draft is component-level state shared by every card,
  // so reset it whenever the expanded card changes — opening continue
  // on a different card must start empty, never inherit the previous
  // card's text (else "send" would route one worker's prompt to
  // another). `continueError` is intentionally left intact so a
  // retry/continue failure stays visible on its card.
  useEffect(() => {
    setContinueText("");
  }, [continueExpandedFor]);

  // Tick relative-time labels once per second so "12s ago" advances.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // The Review Queue is not the default approval surface. It hides when
  // nothing needs attention, keeping the right sidebar calm; mission-level
  // supervisor decisions use the inbox.
  if (orderedIds.length === 0) {
    return null;
  }

  return (
    <div className="review-queue-panel">
      <div className="comms-panel-title">Review Queue ({orderedIds.length})</div>
      <div className="review-queue-list">
        {orderedIds.map((wid) => {
          const worker = workers[wid];
          if (!worker) return null;
          const status = getReviewStatus(wid);
          const isSelected = selectedWorkerId === wid;
          const isFocused = focusedWorkerId === wid;
          const canRetry = VENDOR_CAN_RETRY[worker.vendor];
          const expanded = continueExpandedFor === wid;

          const openDrawer = () => {
            setReviewFocus(wid);
            selectWorker(wid);
          };

          const retry = async () => {
            if (!canRetry || isReplay) return;
            setReviewFocus(wid);
            const r = await commands.retryWorker(wid);
            if (r.status === "error") {
              setContinueExpandedFor(wid);
              setContinueError(`retry failed: ${r.error}`);
            }
          };

          const expandContinue = () => {
            if (!canRetry || isReplay) return;
            setReviewFocus(wid);
            setContinueExpandedFor(wid);
            setContinueError(null);
            queueMicrotask(() => textareaRef.current?.focus());
          };

          const cancelContinue = () => {
            setContinueExpandedFor(null);
            setContinueText("");
            setContinueError(null);
          };

          const submitContinue = async () => {
            if (!continueText.trim() || continueBusy) return;
            setContinueBusy(true);
            setContinueError(null);
            const r = await commands.continueWorker(wid, continueText);
            setContinueBusy(false);
            if (r.status === "error") {
              setContinueError(`continue failed: ${r.error}`);
              return;
            }
            setContinueText("");
            setContinueExpandedFor(null);
          };

          const accept = () => {
            if (isReplay) return;
            setReviewStatus(wid, "accepted");
          };
          const reject = () => {
            if (isReplay) return;
            setReviewStatus(wid, "rejected");
          };

          const m3Tip = `not yet supported for ${worker.vendor} (M3)`;

          return (
            <div
              key={wid}
              className={
                "review-queue-item" +
                (isSelected ? " review-queue-item--selected" : "") +
                (isFocused ? " review-queue-item--focused" : "")
              }
              role="group"
              aria-label={`review card ${worker.shortId}`}
              data-worker-id={wid}
              onClick={() => setReviewFocus(wid)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openDrawer();
                }
              }}
              tabIndex={0}
            >
              <div className="review-queue-item-header">
                <span className="review-queue-item-id">{worker.shortId}</span>
                <span
                  className={`review-queue-item-state review-queue-item-state--${worker.state}`}
                >
                  {worker.state}
                </span>
                <span className="review-queue-item-vendor">{worker.vendor}</span>
                <span className="review-queue-item-time">
                  {relativeTime(worker.spawnedAt, now)}
                </span>
                {status && (
                  <span
                    className={`review-queue-item-status review-queue-item-status--${status}`}
                  >
                    {status}
                  </span>
                )}
              </div>
              {worker.completionSummary && (
                <div className="review-queue-item-summary">
                  {worker.completionSummary.split("\n").slice(0, 2).join(" ")}…
                </div>
              )}
              <div
                className="review-queue-item-actions"
                role="toolbar"
                aria-label="review actions"
              >
                <button
                  type="button"
                  className="review-action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    openDrawer();
                  }}
                  aria-label="open drawer"
                  title="Open drawer (O)"
                >
                  Open
                </button>
                <button
                  type="button"
                  className="review-action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    retry();
                  }}
                  disabled={!canRetry || isReplay}
                  aria-label="retry"
                  title={canRetry ? "Retry (R)" : m3Tip}
                >
                  Retry
                </button>
                <button
                  type="button"
                  className="review-action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    expandContinue();
                  }}
                  disabled={!canRetry || isReplay}
                  aria-label="continue"
                  title={canRetry ? "Continue with prompt (⇧R)" : m3Tip}
                >
                  Continue
                </button>
                <button
                  type="button"
                  className="review-action-btn review-action-btn--accept"
                  onClick={(e) => {
                    e.stopPropagation();
                    accept();
                  }}
                  disabled={isReplay}
                  aria-label="accept"
                  title={isReplay ? "Disabled in replay" : "Accept (A)"}
                >
                  Accept
                </button>
                <button
                  type="button"
                  className="review-action-btn review-action-btn--reject"
                  onClick={(e) => {
                    e.stopPropagation();
                    reject();
                  }}
                  disabled={isReplay}
                  aria-label="reject"
                  title={isReplay ? "Disabled in replay" : "Reject (X)"}
                >
                  Reject
                </button>
              </div>
              {expanded && (
                <div
                  className="review-continue-inline"
                  onClick={(e) => e.stopPropagation()}
                >
                  <textarea
                    ref={textareaRef}
                    className="review-continue-input"
                    placeholder="follow-up prompt…"
                    value={continueText}
                    onChange={(e) => setContinueText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        e.preventDefault();
                        cancelContinue();
                      } else if (
                        e.key === "Enter" &&
                        (e.metaKey || e.ctrlKey)
                      ) {
                        e.preventDefault();
                        submitContinue();
                      }
                    }}
                    disabled={continueBusy}
                    aria-label="continue follow-up prompt"
                  />
                  {continueError ? (
                    <div className="review-continue-error" role="alert">
                      {continueError}
                    </div>
                  ) : null}
                  <div className="review-continue-actions">
                    <button
                      type="button"
                      className="review-continue-cancel"
                      onClick={cancelContinue}
                      disabled={continueBusy}
                    >
                      cancel
                    </button>
                    <button
                      type="button"
                      className="review-continue-send"
                      onClick={submitContinue}
                      disabled={!continueText.trim() || continueBusy}
                    >
                      {continueBusy ? "sending…" : "send →"}
                    </button>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
