// S10 — UnresolvedIssuesList component test. Tests the
// presentation-shape input directly; the wire→view mapper has its
// own test in bindings-shim.test.ts.

import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import UnresolvedIssuesList from "../UnresolvedIssuesList";
import {
  viewForUnresolvedIssue,
  type UnresolvedIssue,
  type UnresolvedIssueView,
} from "../bindings-shim";

const ISSUES: UnresolvedIssueView[] = [
  {
    severity: "warning",
    title: "Scope subscore below threshold",
    detail: "Touched 2 files outside declared scope_paths",
    path: "src/orphan.rs",
  },
  {
    severity: "danger",
    title: "Doc coverage gap",
    detail: "Public symbol added without rustdoc",
    path: "src/types.rs:42",
  },
  {
    severity: "info",
    title: "Lint warning",
    detail: "clippy::needless_borrow at 1 site",
    path: null,
  },
];

describe("UnresolvedIssuesList", () => {
  it("renders one row per issue with title / path / severity glyph / count", () => {
    const { container, getByText } = render(<UnresolvedIssuesList issues={ISSUES} />);
    expect(container.querySelectorAll(".unresolved-issue")).toHaveLength(3);
    expect(getByText(/Scope subscore below threshold/)).toBeTruthy();
    expect(getByText(/Doc coverage gap/)).toBeTruthy();
    expect(getByText(/Lint warning/)).toBeTruthy();
    expect(getByText(/src\/orphan\.rs/)).toBeTruthy();
    expect(getByText(/src\/types\.rs:42/)).toBeTruthy();
    expect(getByText("3")).toBeTruthy(); // count in header
    for (const cls of ["warning", "danger", "info"]) {
      expect(container.querySelector(`.unresolved-issue-glyph--${cls}`)).not.toBeNull();
    }
  });

  it("renders empty-state when issue list is empty", () => {
    const { getByText } = render(<UnresolvedIssuesList issues={[]} />);
    expect(getByText(/no unresolved issues/i)).toBeTruthy();
  });
});

describe("viewForUnresolvedIssue (S9 wire → S10 view mapping)", () => {
  it("maps OpenEscalation to danger", () => {
    const wire: UnresolvedIssue = {
      kind: "open_escalation",
      bound: "scope",
      summary: "out of scope",
    };
    const v = viewForUnresolvedIssue(wire);
    expect(v.severity).toBe("danger");
    expect(v.title).toContain("scope");
    expect(v.detail).toBe("out of scope");
  });

  it("maps RecoveryAttempted to warning with class + action + occurrences", () => {
    const wire: UnresolvedIssue = {
      kind: "recovery_attempted",
      class: "missing_file",
      action_taken: "retry",
      occurrences: 2,
    };
    const v = viewForUnresolvedIssue(wire);
    expect(v.severity).toBe("warning");
    expect(v.detail).toContain("retry");
    expect(v.detail).toContain("missing_file");
    expect(v.detail).toContain("2");
  });

  it("maps ContextBudgetTruncated to info with dropped count + worker id", () => {
    const wire: UnresolvedIssue = {
      kind: "context_budget_truncated",
      dropped_count: 5,
      worker_id: "mock-1",
    };
    const v = viewForUnresolvedIssue(wire);
    expect(v.severity).toBe("info");
    expect(v.detail).toContain("5");
    expect(v.path).toBe("mock-1");
  });

  it("maps SubtaskScrubbed to danger with task index and reason", () => {
    const wire: UnresolvedIssue = {
      kind: "subtask_scrubbed",
      task_index: 3,
      reason: "quality_exhausted",
    };
    const v = viewForUnresolvedIssue(wire);
    expect(v.severity).toBe("danger");
    expect(v.title).toContain("3");
    expect(v.detail).toBe("quality_exhausted");
  });
});
