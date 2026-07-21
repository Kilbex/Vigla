// Phase 1 (G4 measurement clause — see `CLAUDE.md` §2 and the product
// strategy v2 spec §3 Phase 1). One-line strip that surfaces the
// supervisor's current activity at the top of the team view.
//
// The strip is deliberately peripheral:
//   - one line, truncates with CSS ellipsis on overflow
//   - no animation on text change, no hover-reveal, no expand
//   - no icons, no color states, no timestamps, no event kinds
//   - hides itself (`return null`) when the selector yields null
//
// All activity-string derivation lives in `applyMissionEvent`; this
// component is a pure read.

import { useMissionsStore } from "./store";
import { selectSupervisorActivity } from "./store";

const PREFIX = "supervisor:";

export default function SupervisorStrip() {
  const activity = useMissionsStore(selectSupervisorActivity);
  if (!activity) return null;

  // The derived string always begins with "supervisor: " — strip the
  // prefix and render it in muted typography so the activity body
  // carries the focal weight, matching the visual rhythm of
  // `mission-active__status` and `AttentionStrip` items.
  const body = activity.startsWith(`${PREFIX} `)
    ? activity.slice(PREFIX.length + 1)
    : activity;

  return (
    <div
      className="mission-active__supervisor-strip"
      data-testid="supervisor-strip"
      role="status"
      aria-label="supervisor activity"
    >
      <span className="mission-active__supervisor-strip-prefix">
        {PREFIX}
      </span>
      <span className="mission-active__supervisor-strip-body" title={body}>
        {body}
      </span>
    </div>
  );
}
