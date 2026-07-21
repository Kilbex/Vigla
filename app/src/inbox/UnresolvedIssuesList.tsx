// S10 — UnresolvedIssuesList.
//
// Renders the flat presentation list mapped from S9's
// CompletionVerdict.unresolved_issues via `viewForUnresolvedIssue`.
// Pure presentation; the issues array is owned by the upstream
// component (MissionInbox).

import type { UnresolvedIssueView } from "./bindings-shim";

interface UnresolvedIssuesListProps {
  issues: UnresolvedIssueView[];
}

function glyph(severity: UnresolvedIssueView["severity"]): string {
  switch (severity) {
    case "info":
      return "i";
    case "warning":
      return "!";
    case "danger":
      return "X";
  }
}

function glyphClass(severity: UnresolvedIssueView["severity"]): string {
  switch (severity) {
    case "info":
      return "unresolved-issue-glyph--info";
    case "warning":
      return "unresolved-issue-glyph--warning";
    case "danger":
      return "unresolved-issue-glyph--danger";
  }
}

export default function UnresolvedIssuesList({
  issues,
}: UnresolvedIssuesListProps) {
  return (
    <section className="unresolved-issues" aria-label="Unresolved issues">
      <header className="unresolved-issues-header">
        <span>Unresolved</span>
        <span className="unresolved-issues-count">{issues.length}</span>
      </header>
      {issues.length === 0 ? (
        <div className="unresolved-issues-empty">no unresolved issues</div>
      ) : (
        <ul>
          {issues.map((issue) => (
            <li
              key={`${issue.severity}:${issue.title}:${issue.detail ?? ""}:${issue.path ?? ""}`}
              className="unresolved-issue"
              aria-label={`${issue.severity} issue: ${issue.title}`}
            >
              <span
                className={["unresolved-issue-glyph", glyphClass(issue.severity)]
                  .join(" ")}
                aria-hidden="true"
              >
                {glyph(issue.severity)}
              </span>
              <div className="unresolved-issue-body">
                <div className="unresolved-issue-title">{issue.title}</div>
                {issue.detail ? (
                  <div className="unresolved-issue-detail">{issue.detail}</div>
                ) : null}
                {issue.path ? (
                  <div className="unresolved-issue-path">{issue.path}</div>
                ) : null}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
