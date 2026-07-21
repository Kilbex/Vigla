// S10 — AuditBreakdown.
//
// Renders a single audit pass: composite overall score (large
// header), tier label, and one row per sub-scorer (Tests, Scope,
// Regression, Lint) with a progress bar + numeric value. Scorers
// that were skipped (None on the Rust side, null in JSON) render
// dimmed as "skipped".
//
// The component is purely presentational — it parses the
// payload_json string on demand and renders the result. Callsite
// (MissionInbox) reads the latest `supervisor.audit_completed`
// payload off the active mission's event stream and passes the
// `tier` + `payload_json` strings through.

import { useMemo } from "react";

interface AuditBreakdownProps {
  /** Audit tier string from the wire event: "smoke" | "standard"
   *  | "deep". Free-form for forward-compat. */
  tier: string;
  /** Serialised AuditReport (see orchestrator/src/audit/report.rs).
   *  `null` when no audit has fired yet; an empty string is treated
   *  as null. Malformed JSON is surfaced as "audit unavailable"
   *  rather than throwing in the render path. */
  payloadJson: string | null;
}

interface TestPassScore {
  ran: boolean;
  passed: number;
  failed: number;
  skipped: number;
  score: number;
}
interface ScopeScore {
  in_scope: number;
  out_of_scope: number;
  score: number;
}
interface RegressionScore {
  baseline_passed: boolean;
  current_passed: boolean;
  newly_failing: string[];
  newly_passing: string[];
  score: number;
}
interface LintScore {
  rustfmt_clean: boolean | null;
  clippy_warnings: number | null;
  biome_diagnostics: number | null;
  score: number;
}
interface AuditReportShape {
  overall: number;
  test_pass: TestPassScore | null;
  scope: ScopeScore | null;
  regression: RegressionScore | null;
  lint: LintScore | null;
  security_flags: unknown[];
}

function tryParse(payload: string | null): AuditReportShape | null | "error" {
  if (!payload) return null;
  try {
    return JSON.parse(payload) as AuditReportShape;
  } catch {
    return "error";
  }
}

function overallColorClass(overall: number): string {
  if (overall >= 0.8) return "audit-breakdown-overall--good";
  if (overall >= 0.5) return "audit-breakdown-overall--warning";
  return "audit-breakdown-overall--bad";
}

function barColorClass(score: number): string {
  if (score >= 0.8) return "";
  if (score >= 0.5) return "audit-breakdown-row-fill--warning";
  return "audit-breakdown-row-fill--bad";
}

function row(
  name: string,
  score: unknown,
  detail: string | null,
) {
  const scoreValue = readScore(score);
  const skipped = scoreValue === null;
  const value = scoreValue !== null ? scoreValue.toFixed(2) : "skipped";
  const pct = scoreValue !== null ? Math.max(0, Math.min(1, scoreValue)) * 100 : 0;
  const barClass = scoreValue !== null ? barColorClass(scoreValue) : "";
  return (
    <div
      key={name}
      className={[
        "audit-breakdown-row",
        skipped ? "audit-breakdown-row--skipped" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      role="row"
    >
      <span className="audit-breakdown-row-name" role="cell">
        {name}
      </span>
      <div
        className="audit-breakdown-row-bar"
        role="meter"
        aria-label={`${name} audit score`}
        aria-valuemin={0}
        aria-valuemax={1}
        aria-valuenow={scoreValue ?? undefined}
        aria-valuetext={skipped ? "skipped" : `${value} out of 1`}
      >
        <div
          className={["audit-breakdown-row-fill", barClass]
            .filter(Boolean)
            .join(" ")}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="audit-breakdown-row-value" role="cell" title={detail ?? ""}>
        {value}
      </span>
    </div>
  );
}

function readScore(score: unknown): number | null {
  if (!score || typeof score !== "object") return null;
  const value = (score as { score?: unknown }).score;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readNumber(record: unknown, key: string): number | null {
  if (!record || typeof record !== "object") return null;
  const value = (record as Record<string, unknown>)[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readArrayLength(record: unknown, key: string): number | null {
  if (!record || typeof record !== "object") return null;
  const value = (record as Record<string, unknown>)[key];
  return Array.isArray(value) ? value.length : null;
}

function testPassDetail(score: unknown): string | null {
  if (readScore(score) === null) return null;
  const passed = readNumber(score, "passed") ?? 0;
  const failed = readNumber(score, "failed") ?? 0;
  return `${passed} pass / ${failed} fail`;
}

function scopeDetail(score: unknown): string | null {
  if (readScore(score) === null) return null;
  const inScope = readNumber(score, "in_scope") ?? 0;
  const outOfScope = readNumber(score, "out_of_scope") ?? 0;
  return `${inScope} in / ${outOfScope} out`;
}

function regressionDetail(score: unknown): string | null {
  if (readScore(score) === null) return null;
  return `${readArrayLength(score, "newly_failing") ?? 0} newly failing`;
}

export default function AuditBreakdown({ tier, payloadJson }: AuditBreakdownProps) {
  const parsed = useMemo(() => tryParse(payloadJson), [payloadJson]);

  if (parsed === null) {
    return (
      <section className="audit-breakdown" aria-label="Audit breakdown">
        <div className="audit-breakdown-empty">awaiting audit…</div>
      </section>
    );
  }
  if (parsed === "error") {
    return (
      <section className="audit-breakdown" aria-label="Audit breakdown">
        <div className="audit-breakdown-empty">audit unavailable</div>
      </section>
    );
  }

  const overall =
    typeof parsed.overall === "number" && Number.isFinite(parsed.overall)
      ? parsed.overall
      : 0;
  const overallClass = overallColorClass(overall);

  return (
    <section className="audit-breakdown" aria-label="Audit breakdown">
      <header className="audit-breakdown-header">
        <span className={["audit-breakdown-overall", overallClass].join(" ")}>
          {overall.toFixed(2)}
        </span>
        <span className="audit-breakdown-tier">{tier.toUpperCase()}</span>
      </header>
      <div className="audit-breakdown-rows" role="table" aria-label="Audit scorer rows">
        {row(
          "Tests",
          parsed.test_pass,
          testPassDetail(parsed.test_pass),
        )}
        {row("Scope", parsed.scope, scopeDetail(parsed.scope))}
        {row("Regression", parsed.regression, regressionDetail(parsed.regression))}
        {row("Lint", parsed.lint, readScore(parsed.lint) !== null ? "see details" : null)}
      </div>
    </section>
  );
}
