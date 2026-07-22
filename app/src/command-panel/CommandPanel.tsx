import { useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { commands, type HealthDto } from "../bindings";
import { requiresAction } from "../inbox/types";
import { useSurfaceStore } from "../inbox/router";
import MemoryDrawerButton from "../memory/MemoryDrawerButton";
import PinNoteButton from "../memory/PinNoteButton";
import { useMissionsStore } from "../missions/store";
import { isTerminal } from "../missions/types";
import {
  selectGlobalCounters,
  selectIsReplay,
  selectWorkersNeedingReview,
  useOpsStore,
} from "../store";

function formatUptime(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

interface CommandPanelProps {
  onOpenSettings?: () => void;
}

export default function CommandPanel({ onOpenSettings }: CommandPanelProps) {
  const [health, setHealth] = useState<HealthDto | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  const [healthObservedAt, setHealthObservedAt] = useState(() => Date.now());
  const [clock, setClock] = useState(() => Date.now());
  // useShallow: skip the re-render when no displayed counter changed
  // (e.g. selectWorker, exitReplay — store mutations that don't touch
  // any of these five values).
  const counters = useOpsStore(useShallow(selectGlobalCounters));
  const isReplay = useOpsStore(selectIsReplay);
  const exitReplay = useOpsStore((s) => s.exitReplay);
  const setReviewFocus = useOpsStore((s) => s.setReviewFocus);
  const setSurface = useSurfaceStore((s) => s.setSurface);
  const workerNeedsInput = useOpsStore((s) => selectWorkersNeedingReview(s).length);
  const missionAttentionCount = useMissionsStore((s) => {
    const mission = s.active;
    if (!mission || isTerminal(mission.lifecycle)) return 0;
    const cardCount = mission.inbox.filter(requiresAction).length;
    const decisionCount =
      mission.lifecycle === "complete_pending_merge" ||
      mission.lifecycle === "attention"
        ? 1
        : 0;
    return cardCount + decisionCount;
  });
  const terminalMissionVisible = useMissionsStore((s) =>
    s.active ? isTerminal(s.active.lifecycle) : false,
  );
  const attentionCount =
    missionAttentionCount + (terminalMissionVisible ? 0 : workerNeedsInput);
  // Batch 3 (B3.2) — clicking the needs-input chip scrolls the
  // review queue into view and primes keyboard focus on the first
  // card. Snapshot the queue at click-time so we don't subscribe
  // the panel to every queue mutation.
  const peekFirstQueueId = (): string | null => {
    const ids = selectWorkersNeedingReview(useOpsStore.getState());
    return ids.length > 0 ? ids[0] : null;
  };
  const focusFirstNeedsInput = () => {
    const wid = peekFirstQueueId();
    setSurface("inbox");
    if (wid) setReviewFocus(wid);
    window.requestAnimationFrame(() => {
      const panel =
        document.querySelector(".inbox-card--severity-action_required") ??
        document.querySelector(".review-queue-panel") ??
        document.querySelector(".inbox-overview");
      if (panel) panel.scrollIntoView({ behavior: "smooth", block: "nearest" });
    });
  };

  // Event rate sliding history (30 intervals of 2 seconds = last 60 seconds)
  const [eventHistory, setEventHistory] = useState<number[]>(() => Array(30).fill(0));
  const totalEvents = counters.totalEvents;
  const totalEventsRef = useRef(totalEvents);
  useEffect(() => {
    totalEventsRef.current = totalEvents;
  }, [totalEvents]);

  const lastTickEventsRef = useRef(totalEvents);

  useEffect(() => {
    const id = window.setInterval(() => {
      const current = totalEventsRef.current;
      const diff = Math.max(0, current - lastTickEventsRef.current);
      lastTickEventsRef.current = current;
      setEventHistory((prev) => [...prev.slice(1), diff]);
    }, 2000);
    return () => window.clearInterval(id);
  }, []);

  const points = useMemo(() => {
    const maxVal = Math.max(...eventHistory, 1);
    return eventHistory
      .map((val, idx) => {
        const x = (idx / (eventHistory.length - 1)) * 40;
        const y = 10 - (val / maxVal) * 8;
        return `${x},${y}`;
      })
      .join(" ");
  }, [eventHistory]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      commands
        .healthCheck()
        .then((h) => {
          if (cancelled) return;
          setHealth(h);
          setHealthObservedAt(Date.now());
          setHealthErr(null);
        })
        .catch((e) => {
          if (cancelled) return;
          setHealth(null);
          setHealthErr(String(e));
        });
    };
    tick();
    // Health/version are a coarse runtime signal. Poll the backend every ten
    // seconds and advance uptime locally instead of crossing IPC every second.
    const id = window.setInterval(tick, 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    if (!health) return;
    setClock(Date.now());
    const id = window.setInterval(() => setClock(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [health]);

  const displayedUptime = health
    ? health.uptime_ms + Math.max(0, clock - healthObservedAt)
    : null;
  const attentionLabel =
    attentionCount === 1
      ? "1 item needs input"
      : `${attentionCount} items need input`;

  return (
    <header className="command-panel" data-tauri-drag-region>
      <div className="command-panel__identity" data-tauri-drag-region>
        <svg
          className="command-panel-reticle"
          width="14"
          height="14"
          viewBox="0 0 14 14"
          aria-hidden
          focusable="false"
        >
          <circle
            cx="7"
            cy="7"
            r="5.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="0.8"
            opacity="0.7"
          />
          <path
            d="M7 1v2M7 11v2M1 7h2M11 7h2"
            fill="none"
            stroke="currentColor"
            strokeWidth="0.8"
          />
          <circle cx="7" cy="7" r="1.2" fill="currentColor" />
        </svg>
        <span className="brand">Vigla</span>
        {isReplay ? (
          <button
            className="command-panel-history command-panel-channel command-panel-channel--replay"
            onClick={exitReplay}
            title="Return to the live event stream"
            aria-label="History mode active; return to live"
          >
            <span className="command-panel-channel__dot" aria-hidden />
            <span className="command-panel-channel__label">History mode</span>
          </button>
        ) : (
          <span
            className="command-panel-history command-panel-channel command-panel-channel--live"
            title="Live event stream active"
            aria-label="Live event stream active"
            role="status"
          >
            <span className="command-panel-channel__dot" aria-hidden />
            <span className="command-panel-channel__label">Live</span>
          </span>
        )}
      </div>

      <div
        className="command-panel__telemetry"
        role="group"
        aria-label="Mission telemetry"
      >
        <span className="meta command-panel-metric command-panel-metric--active">
          <span className="command-panel-metric__label">Active</span>
          <strong>{counters.active}</strong>
          <span className="meta-faint">/ {counters.total}</span>
        </span>
        <span className="meta command-panel-metric command-panel-events-meta">
          <span className="command-panel-sparkline-wrapper">
            <svg
              className="command-panel-sparkline"
              width="40"
              height="12"
              viewBox="0 0 40 12"
              aria-hidden
            >
              <polyline
                fill="none"
                stroke="currentColor"
                strokeWidth="1.2"
                points={points}
              />
            </svg>
          </span>
          <span className="command-panel-metric__label">Events</span>
          <strong>{counters.totalEvents}</strong>
        </span>
        <span className="meta command-panel-metric command-panel-metric--spend">
          <span className="command-panel-metric__label">Spend</span>
          <strong>${counters.totalSpendUsd.toFixed(3)}</strong>
        </span>
        <span className="meta command-panel-metric command-panel-metric--uptime">
          <span className="command-panel-metric__label">Uptime</span>
          {displayedUptime !== null ? (
            <strong>{formatUptime(displayedUptime)}</strong>
          ) : healthErr ? (
            <span className="mission-review__faint" title={healthErr}>
              n/a
            </span>
          ) : (
            <span className="hud-skeleton">···</span>
          )}
        </span>
      </div>

      {attentionCount > 0 ? (
        <button
          type="button"
          className="meta meta-needs-input"
          onClick={focusFirstNeedsInput}
          aria-label={attentionLabel}
          title="Open items needing input"
        >
          <AttentionIcon />
          <span>{attentionLabel}</span>
        </button>
      ) : null}

      <span className="meta-spacer" data-tauri-drag-region />

      <div className="command-panel__actions">
        <MemoryDrawerButton />
        <PinNoteButton />
        <button
          className="command-panel-history-surface"
          onClick={() => setSurface("history")}
          title="Open recent mission history"
          aria-label="Open mission history"
        >
          History
        </button>
      </div>

      <button
        type="button"
        className={
          "meta meta-faint command-panel-version" +
          (healthErr ? " command-panel-version--err" : "")
        }
        title={healthErr ?? "Open Settings for app and runtime details"}
        onClick={onOpenSettings}
        aria-label="Open app settings and version details"
        disabled={!onOpenSettings}
      >
        <span className="command-panel-version__dot" aria-hidden />
        <span aria-hidden>v</span>
        {health ? (
          health.version
        ) : healthErr ? (
          <span className="mission-review__faint" title={healthErr}>
            n/a
          </span>
        ) : (
          <span className="hud-skeleton">···</span>
        )}
      </button>
    </header>
  );
}

function AttentionIcon() {
  return (
    <svg
      className="command-panel-attention-icon"
      aria-hidden="true"
      viewBox="0 0 16 16"
      focusable="false"
    >
      <path d="M8 2.2 14.2 13H1.8L8 2.2Z" />
      <path d="M8 5.8v3.5M8 11.4v.1" />
    </svg>
  );
}
