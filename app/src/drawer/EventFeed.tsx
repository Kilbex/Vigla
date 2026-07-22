import { useEffect, useMemo, useRef, useState } from "react";
import type { Event, LogLevel } from "../bindings";

const ALL_TYPES = [
  "state_change",
  "log",
  "progress",
  "file_activity",
  "test_result",
  "cost",
  "dependency",
  "completion",
  "failure",
] as const;

type EventType = (typeof ALL_TYPES)[number];

interface EventFeedProps {
  events: Event[];
}

const TOK_FMT = new Intl.NumberFormat("en-US");
const MAX_RENDERED_EVENTS = 500;

function formatCostUsd(usd: number): string {
  return `$${usd.toFixed(usd < 0.01 ? 4 : 3)}`;
}

export function summarize(e: Event): string {
  switch (e.type) {
    case "state_change":
      return `state → ${e.payload.state}${e.payload.from ? ` (from ${e.payload.from})` : ""}${e.payload.note ? ` · ${e.payload.note}` : ""}`;
    case "log":
      return `[${e.payload.level}/${e.payload.stream}] ${e.payload.line}`;
    case "progress":
      return `${e.payload.percent.toFixed(1)}%${e.payload.eta_ms ? ` · eta ${e.payload.eta_ms}ms` : ""}${e.payload.note ? ` · ${e.payload.note}` : ""}`;
    case "file_activity": {
      const op = e.payload.op ?? "edit";
      const path = e.payload.path ?? "(unknown file)";
      const adds = e.payload.lines_added ?? 0;
      const removes = e.payload.lines_removed ?? 0;
      return `${op} ${path} (+${adds}/-${removes})`;
    }
    case "test_result":
      return `${e.payload.suite}: ${e.payload.passed} pass / ${e.payload.failed} fail / ${e.payload.skipped} skip`;
    case "cost":
      return `${formatCostUsd(e.payload.usd ?? 0)} · ${TOK_FMT.format(e.payload.input_tokens ?? 0)} in / ${TOK_FMT.format(e.payload.output_tokens ?? 0)} out`;
    case "dependency":
      return `waiting on ${e.payload.waiting_on.join(", ")} · ${e.payload.reason}`;
    case "completion":
      return `complete · ${e.payload.summary}`;
    case "failure":
      return `FAIL ${e.payload.error}${e.payload.retryable ? " (retryable)" : ""}`;
  }
}

function logLevel(e: Event): LogLevel | null {
  return e.type === "log" ? e.payload.level : null;
}

const LEVEL_RANK: Record<LogLevel, number> = {
  trace: 0,
  debug: 1,
  info: 2,
  warn: 3,
  error: 4,
};

export default function EventFeed({ events }: EventFeedProps) {
  const [search, setSearch] = useState("");
  const [minLevel, setMinLevel] = useState<LogLevel>("trace");
  const [typeFilter, setTypeFilter] = useState<Set<EventType>>(
    () => new Set(ALL_TYPES),
  );
  const [follow, setFollow] = useState(true);
  const [announcement, setAnnouncement] = useState("");
  const tailRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return events.filter((e) => {
      if (!typeFilter.has(e.type as EventType)) return false;
      const lvl = logLevel(e);
      if (lvl && LEVEL_RANK[lvl] < LEVEL_RANK[minLevel]) return false;
      if (!q) return true;
      const summary = `${e.type} ${summarize(e)}`.toLowerCase();
      return summary.includes(q);
    });
  }, [events, search, minLevel, typeFilter]);
  const visible = useMemo(
    () => filtered.slice(-MAX_RENDERED_EVENTS),
    [filtered],
  );
  const latestVisible = visible[visible.length - 1];
  const latestVisibleKey = latestVisible
    ? `${latestVisible.worker_id}:${latestVisible.seq}`
    : "";

  // Announce settled batches, not every streaming row. Continuous output
  // resets the timer and therefore cannot flood a screen reader.
  useEffect(() => {
    const id = window.setTimeout(() => {
      setAnnouncement(
        `${filtered.length} events match the current filters.${
          latestVisible ? ` Latest event #${latestVisible.seq}.` : ""
        }`,
      );
    }, 500);
    return () => window.clearTimeout(id);
  }, [filtered.length, latestVisibleKey]);

  useEffect(() => {
    if (follow && tailRef.current) {
      tailRef.current.scrollIntoView({ block: "end" });
    }
  }, [filtered.length, follow, latestVisibleKey]);

  const toggleType = (t: EventType) => {
    setTypeFilter((prev) => {
      const next = new Set(prev);
      if (next.has(t)) next.delete(t);
      else next.add(t);
      return next;
    });
  };

  return (
    <div className="drawer-feed">
      <div className="drawer-feed-controls">
        <input
          type="search"
          placeholder="search…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="drawer-search"
        />
        <label className="drawer-control">
          <span>level ≥</span>
          <select
            value={minLevel}
            onChange={(e) => setMinLevel(e.target.value as LogLevel)}
          >
            {(["trace", "debug", "info", "warn", "error"] as LogLevel[]).map(
              (l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ),
            )}
          </select>
        </label>
        <label className="drawer-control">
          <input
            type="checkbox"
            checked={follow}
            onChange={(e) => setFollow(e.target.checked)}
          />
          <span>follow</span>
        </label>
      </div>
      <div className="drawer-feed-types">
        {ALL_TYPES.map((t) => (
          <button
            key={t}
            className={
              "drawer-type-pill" +
              (typeFilter.has(t) ? " drawer-type-pill--on" : "")
            }
            onClick={() => toggleType(t)}
            type="button"
          >
            {t}
          </button>
        ))}
      </div>
      <span className="visually-hidden" aria-live="polite">
        {announcement}
      </span>
      <div
        className="drawer-feed-list"
        role="log"
        aria-live="off"
        aria-label="Worker event log"
        tabIndex={0}
      >
        {filtered.length === 0 ? (
          <div className="drawer-empty">no events match</div>
        ) : (
          <>
            {filtered.length > visible.length ? (
              <div className="drawer-feed-limit" role="note">
                Showing the newest {visible.length} of {filtered.length} matching
                events.
              </div>
            ) : null}
            {visible.map((e) => (
              <div key={`${e.worker_id}-${e.seq}`} className="drawer-feed-line">
                <span className="drawer-feed-seq">#{e.seq}</span>
                <span className="drawer-feed-type" data-type={e.type}>
                  {e.type}
                </span>
                <span className="drawer-feed-summary">{summarize(e)}</span>
              </div>
            ))}
          </>
        )}
        <div className="drawer-feed-list__cursor" aria-hidden ref={tailRef}>●</div>
      </div>
    </div>
  );
}
