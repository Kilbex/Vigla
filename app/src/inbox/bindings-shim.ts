// S10 — bindings shim.
//
// Re-exports S9 wire types and adapts the discriminated-union
// `UnresolvedIssue` into a flattened presentation shape that the
// list component consumes. The mapper lives here so the only place
// touched when S9's wire shape evolves is this file — MissionInbox
// and UnresolvedIssuesList stay stable.

import type {
  CompletionVerdict as ImportedVerdict,
  MissionHistoryRow as ImportedHistoryRow,
  RiskBand as ImportedRiskBand,
  UnresolvedIssue as ImportedUnresolvedIssue,
} from "../bindings";

export type CompletionVerdict = ImportedVerdict;
export type MissionHistoryRow = ImportedHistoryRow;
export type RiskBand = ImportedRiskBand;
export type UnresolvedIssue = ImportedUnresolvedIssue;

/** Presentation shape consumed by `UnresolvedIssuesList`. Severity
 *  drives the glyph + color; title is the row header; detail + path
 *  are the secondary lines (either may be null). */
export interface UnresolvedIssueView {
  severity: "info" | "warning" | "danger";
  title: string;
  detail: string | null;
  path: string | null;
}

/** Map an S9 wire `UnresolvedIssue` to the flat view shape. The
 *  severity assignment encodes intent:
 *
 *    * `OpenEscalation` → danger (mission ended with an open
 *      escalation; user must resolve before merge).
 *    * `RecoveryAttempted` → warning (got through it but the
 *      mission was not clean).
 *    * `ContextBudgetTruncated` → info (composer truncated
 *      context; worker still completed).
 *    * `SubtaskScrubbed` → danger (a subtask was thrown away;
 *      the mission's diff is partial).
 */
export function viewForUnresolvedIssue(
  issue: UnresolvedIssue,
): UnresolvedIssueView {
  switch (issue.kind) {
    case "open_escalation":
      return {
        severity: "danger",
        title: `Escalation: ${issue.bound}`,
        detail: issue.summary,
        path: null,
      };
    case "recovery_attempted":
      return {
        severity: "warning",
        title: "Recovery attempted",
        detail: `${issue.action_taken} × ${issue.occurrences} (${issue.class})`,
        path: null,
      };
    case "context_budget_truncated":
      return {
        severity: "info",
        title: "Context truncated",
        detail: `${issue.dropped_count} notes dropped`,
        path: issue.worker_id,
      };
    case "subtask_scrubbed":
      return {
        severity: "danger",
        title: `Subtask ${issue.task_index} scrubbed`,
        detail: issue.reason,
        path: null,
      };
  }
}
