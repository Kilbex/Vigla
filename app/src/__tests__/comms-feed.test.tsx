import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";

// CommsFeed mounts DeployPanel (which probes CLI auth on mount),
// PlaybookPanel (which fires listPlaybooks on every render), and
// SquadPanel. Mock the IPCs so the component renders deterministically.
vi.mock("../bindings", () => ({
  commands: {
    appSettings: vi.fn().mockResolvedValue({
      version: "0.0.1",
      db_path: "",
      repo_root: "",
      mock_harness_path: "",
      mock_harness_present: true,
      l1_quota_mock_enabled: false,
      claude_present: false,
      codex_present: false,
      gemini_present: false,
      antigravity_present: false,
      kiro_present: false,
      copilot_present: false,
      cli_auth: [],
    }),
    startClaudeWorker: vi.fn(),
    startCodexWorker: vi.fn(),
    startGeminiWorker: vi.fn(),
    startMission: vi.fn(),
    checkCliAuth: vi.fn().mockResolvedValue([]),
    openCliLogin: vi.fn(),
    listPlaybooks: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    savePlaybook: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    deletePlaybook: vi.fn().mockResolvedValue({ status: "ok", data: null }),
  },
}));

import CommsFeed from "../comms/CommsFeed";
import { useOpsStore } from "../store";
import { initialReplayState } from "../replay/state";
import { emptyState } from "../store/ingest";

beforeEach(() => {
  vi.clearAllMocks();
  useOpsStore.setState({
    ...emptyState(),
    replay: initialReplayState,
    liveSnapshot: null,
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("CommsFeed timestamps", () => {
  it("ages displayed alert times over time even with no new events", () => {
    vi.useFakeTimers();
    const t0 = new Date("2026-05-09T00:00:00.000Z").getTime();
    vi.setSystemTime(t0);

    useOpsStore.setState({
      alerts: [
        {
          id: "a-1",
          kind: "started",
          workerId: "w-1",
          workerShortId: "w-1",
          title: "spawned",
          detail: null,
          ts: t0,
        },
      ],
    });

    render(<CommsFeed />);
    // diff = 0s → "now".
    expect(screen.getByText("now")).toBeInTheDocument();

    // 30s elapse — the 1s interval fires 30 times. The last setNow
    // wins: now = t0 + 30_000, diff = 30s.
    act(() => {
      vi.advanceTimersByTime(30_000);
    });
    expect(screen.getByText("30s")).toBeInTheDocument();

    // Cross the minute boundary.
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(screen.getByText("1m")).toBeInTheDocument();
  });

  it("ages multiple alerts together as time passes", () => {
    vi.useFakeTimers();
    const t0 = new Date("2026-05-09T00:00:00.000Z").getTime();
    vi.setSystemTime(t0);

    useOpsStore.setState({
      alerts: [
        {
          id: "a-old",
          kind: "started",
          workerId: "w-old",
          workerShortId: "w-old",
          title: "old spawn",
          detail: null,
          ts: t0 - 30_000, // already 30s old at mount
        },
        {
          id: "a-new",
          kind: "completion",
          workerId: "w-new",
          workerShortId: "w-new",
          title: "fresh complete",
          detail: null,
          ts: t0,
        },
      ],
    });

    render(<CommsFeed />);
    // Initial: old → "30s", fresh → "now".
    expect(screen.getByText("30s")).toBeInTheDocument();
    expect(screen.getByText("now")).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(35_000);
    });
    // Old now 65s → "1m"; fresh now 35s.
    expect(screen.getByText("1m")).toBeInTheDocument();
    expect(screen.getByText("35s")).toBeInTheDocument();
  });
});

describe("CommsFeed structure (Step 23)", () => {
  it("mounts the DEPLOY WORKERS panel and no longer carries inline mock spawn buttons", async () => {
    render(<CommsFeed />);
    await waitFor(() =>
      expect(screen.getByText(/^DEPLOY WORKERS$/)).toBeInTheDocument(),
    );
    // SPAWN MOCK heading is gone (moved to Settings → Developer).
    expect(screen.queryByText(/^SPAWN MOCK$/)).not.toBeInTheDocument();
    // The "reset room" button at the top of the comms feed is gone too.
    expect(
      screen.queryByRole("button", { name: /^reset room$/i }),
    ).not.toBeInTheDocument();
  });
});
