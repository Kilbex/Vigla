import type { Event } from "../bindings";

interface StripProps {
  events: Event[];
}

export function FilesStrip({ events }: StripProps) {
  const files = events.filter((e) => e.type === "file_activity");
  if (files.length === 0) {
    return <div className="drawer-empty">no file activity yet</div>;
  }
  return (
    <ul className="drawer-strip" tabIndex={0} aria-label="File activity">
      {files.map((e) => {
        if (e.type !== "file_activity") return null;
        const adds = e.payload.lines_added ?? 0;
        const removes = e.payload.lines_removed ?? 0;
        return (
          <li key={`${e.worker_id}-${e.seq}`} className="drawer-strip-row">
            <span className={`drawer-op drawer-op--${e.payload.op}`}>
              {e.payload.op}
            </span>
            <span className="drawer-strip-path" title={e.payload.path}>
              {e.payload.path}
            </span>
            <span className="drawer-strip-stats">
              {adds > 0 ? <span className="drawer-add">+{adds}</span> : null}
              {removes > 0 ? (
                <span className="drawer-rm">−{removes}</span>
              ) : null}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

export function TestsStrip({ events }: StripProps) {
  const tests = events.filter((e) => e.type === "test_result");
  if (tests.length === 0) {
    return <div className="drawer-empty">no test runs yet</div>;
  }
  return (
    <ul className="drawer-strip" tabIndex={0} aria-label="Test runs">
      {tests.map((e) => {
        if (e.type !== "test_result") return null;
        const r = e.payload;
        const failing = r.failed > 0;
        return (
          <li key={`${e.worker_id}-${e.seq}`} className="drawer-strip-row">
            <span className="drawer-strip-suite">{r.suite}</span>
            <span className="drawer-strip-stats">
              <span className="drawer-pass">{r.passed} pass</span>
              <span className={failing ? "drawer-fail" : "drawer-zero"}>
                {r.failed} fail
              </span>
              <span className="drawer-skip">{r.skipped} skip</span>
              <span className="drawer-strip-duration">{r.duration_ms}ms</span>
            </span>
            {failing && r.failures && r.failures.length > 0 ? (
              <ul className="drawer-failures">
                {r.failures.map((f) => (
                  <li key={f.name} className="drawer-failure" title={f.message}>
                    <span className="drawer-failure-name">{f.name}</span>
                    {f.file ? (
                      <span className="drawer-failure-file">
                        {f.file}
                        {f.line ? `:${f.line}` : ""}
                      </span>
                    ) : null}
                    <div className="drawer-failure-msg">{f.message}</div>
                  </li>
                ))}
              </ul>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

export function CostStrip({ events }: StripProps) {
  const costs = events.filter((e) => e.type === "cost");
  let total = 0;
  let inTotal = 0;
  let outTotal = 0;
  let cacheReadTotal = 0;
  for (const e of costs) {
    if (e.type !== "cost") continue;
    total += e.payload.usd;
    inTotal += e.payload.input_tokens;
    outTotal += e.payload.output_tokens;
    cacheReadTotal += e.payload.cache_read_tokens ?? 0;
  }
  if (costs.length === 0) {
    return <div className="drawer-empty">no cost events yet</div>;
  }
  return (
    <div className="drawer-cost">
      <div className="drawer-cost-totals">
        <div>
          <span className="drawer-cost-label">spend</span>
          <span className="drawer-cost-value">${total.toFixed(4)}</span>
        </div>
        <div>
          <span className="drawer-cost-label">input</span>
          <span className="drawer-cost-value">{inTotal.toLocaleString()}</span>
        </div>
        <div>
          <span className="drawer-cost-label">output</span>
          <span className="drawer-cost-value">{outTotal.toLocaleString()}</span>
        </div>
        {cacheReadTotal > 0 ? (
          <div>
            <span className="drawer-cost-label">cache read</span>
            <span className="drawer-cost-value">
              {cacheReadTotal.toLocaleString()}
            </span>
          </div>
        ) : null}
      </div>
      <ul className="drawer-strip" tabIndex={0} aria-label="Cost events">
        {costs.map((e) => {
          if (e.type !== "cost") return null;
          return (
            <li key={`${e.worker_id}-${e.seq}`} className="drawer-strip-row">
              <span className="drawer-strip-suite">{e.payload.model ?? "—"}</span>
              <span className="drawer-strip-stats">
                <span className="drawer-pass">+${e.payload.usd.toFixed(4)}</span>
                <span className="drawer-zero">
                  {e.payload.input_tokens} in / {e.payload.output_tokens} out
                </span>
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
