// Row 4 (R6) — live countdown for a quota-paused mission. Re-renders
// once a second so the paused mission shows "resumes in mm:ss" rather
// than reading as hung. The formatting is delegated to the pure
// `formatTimeToResume`; this component owns only the per-second tick.

import { useEffect, useState } from "react";
import { formatTimeToResume } from "./quotaPause";

interface QuotaCountdownProps {
  /** Unix-ms timestamp the vendor quota window is estimated to reopen. */
  resumeAtMs: number;
}

export default function QuotaCountdown({ resumeAtMs }: QuotaCountdownProps) {
  const [nowMs, setNowMs] = useState<number>(() => Date.now());

  useEffect(() => {
    const initialNow = Date.now();
    setNowMs(initialNow);
    if (initialNow >= resumeAtMs) return;

    const id = window.setInterval(() => {
      const now = Date.now();
      setNowMs(now);
      if (now >= resumeAtMs) window.clearInterval(id);
    }, 1000);
    return () => window.clearInterval(id);
  }, [resumeAtMs]);

  return (
    <span className="quota-countdown" role="timer" aria-live="polite">
      {formatTimeToResume(resumeAtMs, nowMs)}
    </span>
  );
}
