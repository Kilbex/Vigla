// Row 4 (R6) — QuotaCountdown renders a live, ticking time-to-resume
// for a paused mission. Fake timers control the clock so the tick is
// deterministic.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render } from "@testing-library/react";
import QuotaCountdown from "../QuotaCountdown";

const NOW = 1_716_000_000_000;

describe("QuotaCountdown", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the time remaining until the quota window reopens", () => {
    const { getByRole } = render(<QuotaCountdown resumeAtMs={NOW + 90_000} />);
    expect(getByRole("timer").textContent).toBe("resumes in 1m 30s");
  });

  it("ticks down as time advances", () => {
    const { getByRole } = render(<QuotaCountdown resumeAtMs={NOW + 90_000} />);
    act(() => {
      vi.advanceTimersByTime(31_000);
    });
    expect(getByRole("timer").textContent).toBe("resumes in 59s");
  });

  it("stops its interval once the resume window arrives", () => {
    render(<QuotaCountdown resumeAtMs={NOW + 2_000} />);
    expect(vi.getTimerCount()).toBe(1);
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(vi.getTimerCount()).toBe(0);
  });
});
