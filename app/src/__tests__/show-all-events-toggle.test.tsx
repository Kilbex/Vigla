// Drive the App-shell swap between InboxOverview and CommsFeed
// through the useShowAllEvents preference. Verifies the default
// path renders InboxOverview and the toggle restores CommsFeed.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";

vi.mock("../bindings", () => ({
  commands: {
    healthCheck: vi.fn().mockResolvedValue({
      version: "0.0.1",
      uptime_ms: 0,
    }),
    appSettings: vi.fn().mockResolvedValue({
      version: "0.0.1",
      db_path: "",
      repo_root: "",
      mock_harness_path: "",
      mock_harness_present: false,
      l1_quota_mock_enabled: false,
      claude_present: false,
      codex_present: false,
      gemini_present: false,
      antigravity_present: false,
      kiro_present: false,
      copilot_present: false,
      cli_auth: [],
    }),
    listPlaybooks: vi.fn().mockResolvedValue({ status: "ok", data: [] }),
    savePlaybook: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    deletePlaybook: vi.fn().mockResolvedValue({ status: "ok", data: null }),
    startMission: vi.fn(),
    checkCliAuth: vi.fn().mockResolvedValue([]),
    openCliLogin: vi.fn(),
    missionEventVisibility: vi.fn().mockResolvedValue({ kind: "internal" }),
    surfaceInboxNotification: vi.fn().mockResolvedValue(undefined),
  },
  events: {
    workerEvent: { listen: vi.fn().mockResolvedValue(() => {}) },
    missionEventDto: { listen: vi.fn().mockResolvedValue(() => {}) },
  },
}));

import App from "../App";
import { useSurfaceStore } from "../inbox/router";
import { setShowAllEvents } from "../settings/preferences";

beforeEach(() => {
  setShowAllEvents(false);
  useSurfaceStore.setState({ surface: "inbox", detail: null });
});

describe("Show all events toggle", () => {
  it("renders InboxOverview by default", async () => {
    render(<App />);
    expect(await screen.findByRole("complementary", { name: /inbox/i })).toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: /comms feed/i })).not.toBeInTheDocument();
  });

  it("renders CommsFeed when surface=ops_room and toggle is on", async () => {
    // S10: showAllEvents alone no longer flips to CommsFeed — the
    // surface must also be ops_room (the ⌘2 keyboard binding does
    // both transitions together).
    act(() => {
      setShowAllEvents(true);
      useSurfaceStore.setState({ surface: "ops_room", detail: null });
    });
    render(<App />);
    expect(await screen.findByRole("complementary", { name: /comms feed/i })).toBeInTheDocument();
  });
});
