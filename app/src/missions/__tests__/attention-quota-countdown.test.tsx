// Row 4 (R6) — the paused-mission attention row must show the live
// countdown, and only for `mission_paused` items. Renders the
// AttentionStrip directly (pure presentation) under fake timers.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { AttentionStrip } from "../MissionActiveView";
import type { AttentionItem } from "../types";

const NOW = 1_716_000_000_000;
const TS = "2026-06-01T00:00:00.000Z";

describe("AttentionStrip — quota pause countdown", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders a live countdown for a mission_paused item", () => {
    const items: AttentionItem[] = [
      {
        kind: "mission_paused",
        severity: "soft",
        summary: "Paused — waiting on claude quota to reopen",
        surfacedAt: TS,
        resumeAtMs: NOW + 90_000,
      },
    ];
    const { getByRole, getByText } = render(<AttentionStrip items={items} />);
    expect(getByText(/Paused — waiting on claude quota/)).toBeTruthy();
    expect(getByRole("timer").textContent).toBe("resumes in 1m 30s");
  });

  it("does not render a countdown for non-paused attention items", () => {
    const items: AttentionItem[] = [
      {
        kind: "mission_complete",
        severity: "soft",
        summary: "Mission complete",
        surfacedAt: TS,
      },
    ];
    const { queryByRole } = render(<AttentionStrip items={items} />);
    expect(queryByRole("timer")).toBeNull();
  });
});
