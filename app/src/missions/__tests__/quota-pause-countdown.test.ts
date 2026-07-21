// Row 4 (R6) — the pure countdown formatter behind the paused-mission
// "resumes in mm:ss" readout. Kept pure (now passed in) so it tests
// deterministically without fake timers.

import { describe, expect, it } from "vitest";
import { formatTimeToResume } from "../quotaPause";

const NOW = 1_716_000_000_000;

describe("formatTimeToResume", () => {
  it("formats minutes and seconds when >= 60s out", () => {
    expect(formatTimeToResume(NOW + 252_000, NOW)).toBe("resumes in 4m 12s");
  });

  it("formats seconds only when < 60s out", () => {
    expect(formatTimeToResume(NOW + 45_000, NOW)).toBe("resumes in 45s");
  });

  it("rounds down to whole seconds", () => {
    expect(formatTimeToResume(NOW + 45_900, NOW)).toBe("resumes in 45s");
  });

  it("shows 'resuming…' once the resume time has reached or passed now", () => {
    expect(formatTimeToResume(NOW - 1_000, NOW)).toBe("resuming…");
    expect(formatTimeToResume(NOW, NOW)).toBe("resuming…");
    expect(formatTimeToResume(NOW + 500, NOW)).toBe("resuming…");
  });
});
