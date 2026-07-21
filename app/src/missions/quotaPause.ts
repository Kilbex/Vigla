// Row 4 (R6) — paused-mission countdown helper. A vendor quota window
// closed; the wake-up task resumes the mission automatically at the
// estimated reset time. This formats the time remaining so the paused
// mission reads as "resumes in mm:ss" rather than looking hung.

/**
 * Human-readable time-to-resume. `nowMs` is injected (not read from
 * the clock) so the formatter is pure and testable. Returns
 * "resuming…" once the estimated resume time has reached or passed
 * now — at that point the wake-up task is firing and an exact count
 * is meaningless.
 */
export function formatTimeToResume(resumeAtMs: number, nowMs: number): string {
  const totalSec = Math.floor((resumeAtMs - nowMs) / 1000);
  if (totalSec <= 0) return "resuming…";
  const minutes = Math.floor(totalSec / 60);
  const seconds = totalSec % 60;
  return minutes > 0
    ? `resumes in ${minutes}m ${seconds}s`
    : `resumes in ${seconds}s`;
}
