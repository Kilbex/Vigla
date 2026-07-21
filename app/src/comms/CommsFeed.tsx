import { useEffect, useState } from "react";
import { selectAlerts, useOpsStore } from "../store";
import type { Alert } from "../store/types";
import DeployPanel from "./DeployPanel";
import ReviewQueue from "./ReviewQueue";
import SquadPanel from "./SquadPanel";

function alertGlyph(kind: Alert["kind"]): string {
  switch (kind) {
    case "completion":
      return "✓";
    case "failure":
      return "✕";
    case "blocked":
      return "⏸";
    case "unblocked":
      return "▶";
    case "started":
      return "+";
    case "seq_gap":
      return "⚠";
  }
}

function formatRelativeTime(ts: number, now: number): string {
  const diff = Math.max(0, Math.floor((now - ts) / 1000));
  if (diff < 5) return "now";
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  return `${Math.floor(diff / 3600)}h`;
}

export default function CommsFeed() {
  const alerts = useOpsStore(selectAlerts);
  // Drive relative-time labels via state so quiescent rooms still age
  // their cards. Computing `now` only at render time froze every age
  // at the value captured during the last alerts/busy/status change —
  // a 10-minute-old "now" badge actively misleads the operator.
  const [now, setNow] = useState<number>(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  return (
    <aside className="comms-feed" aria-label="Comms feed">
      <DeployPanel />
      <div className="comms-divider" />
      <div className="comms-actions">
        <SquadPanel />
      </div>
      <div className="comms-divider" />
      <ReviewQueue />
      <div className="comms-divider" />
      <div className="comms-feed-list">
        {alerts.length === 0 ? (
          <div className="comms-empty">no signals yet</div>
        ) : (
          alerts.map((a) => (
            <div key={a.id} className={`comms-card comms-card--${a.kind}`}>
              <div className="comms-card-row">
                <span className="comms-card-glyph">{alertGlyph(a.kind)}</span>
                <span className="comms-card-callsign">{a.workerShortId}</span>
                <span className="comms-card-title">{a.title}</span>
                <span className="comms-card-time">
                  {formatRelativeTime(a.ts, now)}
                </span>
              </div>
              {a.detail ? (
                <div className="comms-card-detail" title={a.detail}>
                  {a.detail}
                </div>
              ) : null}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
