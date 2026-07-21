import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../bindings", () => ({
  commands: {
    resolveMission: vi.fn(),
  },
}));

import MissionReviewOutcome, {
  formatTestsRow,
  isTestsRowFallback,
} from "../MissionReviewOutcome";
import { commands } from "../../bindings";
import type { ActiveMission } from "../types";

function makeMission(overrides: Partial<ActiveMission> = {}): ActiveMission {
  return {
    id: "msn-test-0001",
    spec: {
      title: "Test mission",
      objective: "obj",
      target_ref: "main",
      tests: null,
      supervisor_model: "claude",
      worker_model: null,
      worker_count: null,
      confirm_plan: null,
    } as ActiveMission["spec"],
    lifecycle: "complete_pending_merge",
    startedAt: "2026-05-26T14:00:00Z",
    updatedAt: "2026-05-26T14:30:00Z",
    statusLine: "done",
    progressPercent: 100,
    tasks: [
      {
        index: 0,
        title: "t0",
        description: null,
        status: "integrated",
        assignedWorkerId: "wkr-1",
        integrationSha: "abc",
        snapshotTag: "vigla/pre-merge/msn-test-0001",
      },
    ],
    workers: {},
    testsPassed: null,
    completionSummary: "done",
    filesChanged: 0,
    resolution: null,
    abortReason: null,
    attention: [],
    planGeneration: 0,
    planOverview: null,
    planTechStack: null,
    planEnvelopeFit: null,
    supervisorActivity: null,
    lastExtensionDirective: null,
    lastExtensionAt: null,
    audit: null,
    auditPayloadJson: null,
    verdict: null,
    inbox: [],
    ...overrides,
  };
}

function workerWithFiles(files: string[]) {
  return {
    "wkr-1": {
      id: "wkr-1",
      taskIndex: 0,
      taskTitle: "t0",
      status: "integrated" as const,
      latestProgress: null,
      submittedFiles: files,
    },
  };
}

describe("MissionReviewOutcome — files row", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("per-worker submittedFiles win over filesChanged count", () => {
    const mission = makeMission({
      workers: workerWithFiles(["src/a.ts", "src/b.ts"]),
      filesChanged: 99,
    });
    render(<MissionReviewOutcome mission={mission} />);
    expect(screen.getByText("src/a.ts")).toBeTruthy();
    expect(screen.getByText("src/b.ts")).toBeTruthy();
    expect(screen.queryByText(/99 files changed/i)).toBeNull();
    expect(screen.queryByText(/No files reported/i)).toBeNull();
  });

  it("falls back to filesChanged when workers empty", () => {
    const mission = makeMission({ filesChanged: 7 });
    render(<MissionReviewOutcome mission={mission} />);
    expect(
      screen.getByText("7 files changed (details unavailable)"),
    ).toBeTruthy();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByText(/No files reported/i)).toBeNull();
  });

  it("renders the empty state when workers empty and filesChanged is 0", () => {
    const mission = makeMission({ filesChanged: 0 });
    render(<MissionReviewOutcome mission={mission} />);
    expect(screen.getByText("No files reported.")).toBeTruthy();
  });
});

describe("MissionReviewOutcome — tasks row (integrated count)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function tasksRowText(mission: ActiveMission): string {
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    const rows = container.querySelectorAll(".mission-review__meta-row");
    const tasksRow = Array.from(rows).find(
      (r) => r.querySelector("dt")?.textContent === "Tasks",
    ) as HTMLElement | undefined;
    expect(tasksRow).toBeTruthy();
    return tasksRow!.querySelector("dd")!.textContent ?? "";
  }

  it("counts only integrated tasks, not the total, when a task failed", () => {
    // Reachable via the arbiter-escalation → attention surface: one
    // task integrated, one escalated to `failed`. The row must report
    // the integrated subset, not the total task count.
    const mission = makeMission({
      lifecycle: "attention",
      tasks: [
        {
          index: 0,
          title: "t0",
          description: null,
          status: "integrated",
          assignedWorkerId: "wkr-1",
          integrationSha: "abc",
          snapshotTag: "vigla/pre-merge/msn-test-0001",
        },
        {
          index: 1,
          title: "t1",
          description: null,
          status: "failed",
          assignedWorkerId: "wkr-2",
          integrationSha: null,
          snapshotTag: null,
        },
      ],
    });
    expect(tasksRowText(mission)).toBe("1/2 integrated");
  });

  it("reports all integrated when every task integrated", () => {
    // Default makeMission has a single integrated task.
    expect(tasksRowText(makeMission())).toBe("1/1 integrated");
  });
});

describe("MissionReviewOutcome — tests row", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("legacy testsPassed=true wins over an audit payload claiming failures", () => {
    const mission = makeMission({
      testsPassed: true,
      auditPayloadJson: JSON.stringify({
        test_pass: { ran: true, passed: 0, failed: 5, skipped: 0, score: 0 },
      }),
    });
    expect(formatTestsRow(mission)).toBe("passing");
    render(<MissionReviewOutcome mission={mission} />);
    expect(screen.getByText("passing")).toBeTruthy();
    expect(screen.queryByText(/5 failed/)).toBeNull();
  });

  it("falls back to audit test_pass numbers when legacy verdict is null", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: JSON.stringify({
        test_pass: {
          ran: true,
          passed: 18,
          failed: 1,
          skipped: 2,
          score: 0.94,
        },
      }),
    });
    expect(formatTestsRow(mission)).toBe("18 passed · 1 failed · 2 skipped");
    render(<MissionReviewOutcome mission={mission} />);
    expect(screen.getByText("18 passed · 1 failed · 2 skipped")).toBeTruthy();
  });

  it("omits the skipped clause when skipped is zero", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: JSON.stringify({
        test_pass: {
          ran: true,
          passed: 18,
          failed: 1,
          skipped: 0,
          score: 0.94,
        },
      }),
    });
    expect(formatTestsRow(mission)).toBe("18 passed · 1 failed");
  });

  it("renders 'no tests run' when test_pass.ran is false", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: JSON.stringify({
        test_pass: {
          ran: false,
          passed: 0,
          failed: 0,
          skipped: 0,
          score: 0,
        },
      }),
    });
    expect(formatTestsRow(mission)).toBe("no tests run");
  });

  it("falls back to 'no test data' when auditPayloadJson is malformed", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: "not json",
    });
    expect(formatTestsRow(mission)).toBe("no test data");
  });

  it("falls back to 'no test data' when audit payload has no test_pass block", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: JSON.stringify({ overall: 0.7 }),
    });
    expect(formatTestsRow(mission)).toBe("no test data");
  });

  it("falls back to 'no test data' when auditPayloadJson is null", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: null,
    });
    expect(formatTestsRow(mission)).toBe("no test data");
  });
});

describe("MissionReviewOutcome — P2-14 tests row n/a copy", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders 'no test data' (dimmed, no em-dash) when both legacy and audit are empty", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: null,
    });
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    // Find the Tests row by walking the meta list.
    const rows = container.querySelectorAll(".mission-review__meta-row");
    const testsRow = Array.from(rows).find(
      (r) => r.querySelector("dt")?.textContent === "Tests",
    ) as HTMLElement | undefined;
    expect(testsRow).toBeTruthy();
    const dd = testsRow!.querySelector("dd")!;
    expect(dd.textContent).toBe("no test data");
    const span = dd.querySelector("span");
    expect(span).not.toBeNull();
    expect(span!.classList.contains("mission-review__faint")).toBe(true);
    // The literal em-dash must not appear anywhere in this row.
    expect(dd.textContent).not.toContain("—");
  });

  it("'passing' (legacy testsPassed===true) still wins and is not dimmed", () => {
    const mission = makeMission({ testsPassed: true });
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    expect(formatTestsRow(mission)).toBe("passing");
    const rows = container.querySelectorAll(".mission-review__meta-row");
    const testsRow = Array.from(rows).find(
      (r) => r.querySelector("dt")?.textContent === "Tests",
    ) as HTMLElement | undefined;
    const dd = testsRow!.querySelector("dd")!;
    expect(dd.textContent).toBe("passing");
    const span = dd.querySelector("span");
    expect(span!.classList.contains("mission-review__faint")).toBe(false);
  });

  it("'FAILING' (legacy testsPassed===false) still wins and is not dimmed", () => {
    const mission = makeMission({ testsPassed: false });
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    expect(formatTestsRow(mission)).toBe("FAILING");
    const rows = container.querySelectorAll(".mission-review__meta-row");
    const testsRow = Array.from(rows).find(
      (r) => r.querySelector("dt")?.textContent === "Tests",
    ) as HTMLElement | undefined;
    const dd = testsRow!.querySelector("dd")!;
    expect(dd.textContent).toBe("FAILING");
    const span = dd.querySelector("span");
    expect(span!.classList.contains("mission-review__faint")).toBe(false);
  });

  it("audit-derived numbers still render (regression for P0-3 fallback)", () => {
    const mission = makeMission({
      testsPassed: null,
      auditPayloadJson: JSON.stringify({
        test_pass: { ran: true, passed: 4, failed: 0, skipped: 0, score: 1 },
      }),
    });
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    expect(formatTestsRow(mission)).toBe("4 passed · 0 failed");
    const rows = container.querySelectorAll(".mission-review__meta-row");
    const testsRow = Array.from(rows).find(
      (r) => r.querySelector("dt")?.textContent === "Tests",
    ) as HTMLElement | undefined;
    const dd = testsRow!.querySelector("dd")!;
    expect(dd.textContent).toBe("4 passed · 0 failed");
    // Audit-derived numbers are real data, not n/a.
    const span = dd.querySelector("span");
    expect(span!.classList.contains("mission-review__faint")).toBe(false);
  });

  it("isTestsRowFallback() identifies the n/a copy only", () => {
    expect(isTestsRowFallback("no test data")).toBe(true);
    expect(isTestsRowFallback("passing")).toBe(false);
    expect(isTestsRowFallback("FAILING")).toBe(false);
    expect(isTestsRowFallback("4 passed · 0 failed")).toBe(false);
    expect(isTestsRowFallback("—")).toBe(false);
  });
});

describe("MissionReviewOutcome — action buttons (P1-10/P1-11)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (commands.resolveMission as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: null,
    });
  });

  it("offers only dispositions the runtime can complete", () => {
    const mission = makeMission();
    const { container } = render(<MissionReviewOutcome mission={mission} />);
    const actions = container.querySelector(".mission-review__actions");
    expect(actions).not.toBeNull();
    const labels = Array.from(actions!.querySelectorAll("button")).map(
      (b) => b.textContent ?? "",
    );
    expect(labels).toEqual(["Discard", "Merge"]);
  });

  it("disables Merge when no task integrated", () => {
    const mission = makeMission({
      lifecycle: "attention",
      tasks: [
        {
          index: 0,
          title: "t0",
          description: null,
          status: "failed",
          assignedWorkerId: "wkr-1",
          integrationSha: null,
          snapshotTag: null,
        },
      ],
    });
    render(<MissionReviewOutcome mission={mission} />);

    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
    expect(
      screen.getByText(/nothing was integrated; discard this mission/i),
    ).toBeTruthy();
  });

  it("Discard button uses the tertiary variant, not secondary", () => {
    const mission = makeMission();
    render(<MissionReviewOutcome mission={mission} />);
    const discard = screen.getByRole("button", { name: "Discard" });
    expect(discard.classList.contains("mission-form__button--tertiary")).toBe(
      true,
    );
    expect(discard.classList.contains("mission-form__button--secondary")).toBe(
      false,
    );
  });

  it("does not expose the reserved Extend wire action", () => {
    const mission = makeMission();
    render(<MissionReviewOutcome mission={mission} />);
    expect(screen.queryByRole("button", { name: /continue|extend/i })).toBeNull();
    expect(screen.queryByLabelText(/directive/i)).toBeNull();
  });
});
