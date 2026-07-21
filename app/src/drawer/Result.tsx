import type { Event } from "../bindings";
import type { WorkerSnapshot } from "../store/types";

interface ResultProps {
  worker: WorkerSnapshot;
  events: Event[];
}

const CONTEXT_LIMIT = 8;

/// "Result" tab — surfaces the final completion or failure summary for
/// done/failed workers without forcing the user into the chronological
/// event feed. Pure projection of the worker snapshot + bounded event
/// history kept by the store.
export default function Result({ worker, events }: ResultProps) {
  const isFailed = worker.state === "failed";
  const summary = isFailed ? worker.failureSummary : worker.completionSummary;

  const context = events
    .filter(
      (e) =>
        e.type === "log" &&
        (e.payload.level === "info" || e.payload.level === "warn"),
    )
    .slice(-CONTEXT_LIMIT);

  if (!summary && context.length === 0) {
    return (
      <div className="drawer-empty">
        no result text captured — see feed for full transcript
      </div>
    );
  }

  return (
    <div className="drawer-result">
      {summary ? (
        <section
          className={
            "drawer-result-summary" +
            (isFailed ? " drawer-result-summary--failed" : "")
          }
          aria-label={isFailed ? "failure summary" : "completion summary"}
        >
          {summary}
        </section>
      ) : null}
      {context.length > 0 ? (
        <section
          className="drawer-result-context"
          aria-label="recent log context"
        >
          <header className="drawer-result-context-head">recent log</header>
          <ul className="drawer-result-context-list">
            {context.map((e) => {
              if (e.type !== "log") return null;
              return (
                <li key={e.seq} className="drawer-result-context-row">
                  <span className="drawer-result-context-tag">
                    [{e.payload.level}/{e.payload.stream}]
                  </span>
                  <span className="drawer-result-context-line">
                    {e.payload.line}
                  </span>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
