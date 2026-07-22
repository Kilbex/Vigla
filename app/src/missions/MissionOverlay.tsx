// MSV U1 — the overlay container.
//
// Lifecycle-driven; has no entry point of its own. The Deploy
// Workers panel (`app/src/comms/DeployPanel.tsx`) is the single
// user-facing start surface; calling `commands.startMission` from
// there causes a `mission.created` event to flow through the store,
// which is when this overlay becomes visible.
//
// Renders nothing when no mission is active. Otherwise switches
// between active view / review outcome / terminal close based on
// lifecycle.

import { useEffect, useMemo, useState } from "react";
import { commands } from "../bindings";
import MissionActiveView from "./MissionActiveView";
import MissionPlanPreview from "./MissionPlanPreview";
import MissionReviewOutcome from "./MissionReviewOutcome";
import MissionTerminalOutcome from "./MissionTerminalOutcome";
import { useSurfaceStore } from "../inbox/router";
import { useMissionsStore } from "./store";
import { selectActiveMission, selectMissionLifecycle } from "./store";
import { isTerminal } from "./types";

function formatElapsed(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}m ${String(s).padStart(2, "0")}s`;
}

export default function MissionOverlay() {
  const mission = useMissionsStore(selectActiveMission);
  const lifecycle = useMissionsStore(selectMissionLifecycle);
  const dismissTerminalOverlay = useMissionsStore((s) => s.dismissTerminalOverlay);
  const applyRevertOutcome = useMissionsStore((s) => s.applyRevertOutcome);
  const openMission = useSurfaceStore((s) => s.openMission);

  const [now, setNow] = useState(() => Date.now());
  const isExecuting =
    !!mission &&
    lifecycle !== null &&
    !isTerminal(lifecycle) &&
    lifecycle !== "complete_pending_merge" &&
    lifecycle !== "attention" &&
    lifecycle !== "pending_plan_approval";

  useEffect(() => {
    if (!isExecuting) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [isExecuting]);

  const elapsed = useMemo(() => {
    if (!mission) return undefined;
    const startedMs = Date.parse(mission.startedAt);
    if (!Number.isFinite(startedMs)) return undefined;
    return formatElapsed(Math.max(0, now - startedMs));
  }, [mission, now]);

  const showTerminalCondition =
    !!mission && lifecycle !== null && isTerminal(lifecycle);

  // Esc dismisses the overlay only on the terminal screen — the
  // mission is already done, so closing it is purely cosmetic.
  // Active / complete-pending-merge intentionally do NOT respond
  // to Esc: aborting or resolving a mission requires an explicit
  // click so the user can't lose work by accident.
  useEffect(() => {
    if (!showTerminalCondition) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        dismissTerminalOverlay();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dismissTerminalOverlay, showTerminalCondition]);

  const [abortError, setAbortError] = useState<string | null>(null);

  if (!mission || lifecycle === null) {
    return null;
  }

  const showActive = isExecuting;
  const showPendingPlanApproval = lifecycle === "pending_plan_approval";
  // Phase 1 (decisions.md entry 7): `attention` (sub-supervisor-refused)
  // also surfaces the resolve outcome surface so the user can pick
  // merge or discard without leaving the mission card.
  const showCompletePending =
    lifecycle === "complete_pending_merge" || lifecycle === "attention";
  const showTerminal = isTerminal(lifecycle);

  const handleAbort = async () => {
    setAbortError(null);
    try {
      const result = await commands.abortMission();
      if (result.status === "error") {
        setAbortError(result.error);
      }
    } catch (e) {
      setAbortError(e instanceof Error ? e.message : String(e));
    }
  };

  // The overlay only stays a true modal during terminal dispositions
  // (merged / discarded / historical extended / aborted). Through executing /
  // pending_plan_approval / complete_pending_merge / attention /
  // paused, render the same card as an anchored, non-blocking panel
  // so the user can still reach the inbox right-rail, ops-room
  // workers, history surface, and Settings dialog without first
  // committing a Merge/Discard/Abort.
  const isModal = isTerminal(lifecycle);
  const rootClassName = [
    "mission-overlay",
    isModal ? "mission-overlay--modal" : "mission-overlay--anchored",
    showPendingPlanApproval ? "mission-overlay--plan-review" : null,
  ]
    .filter(Boolean)
    .join(" ");
  const rootAriaProps = isModal
    ? {
        role: "dialog" as const,
        "aria-modal": true,
        "aria-label": mission.spec.title,
      }
    : {
        role: "complementary" as const,
        "aria-label": mission.spec.title,
      };

  return (
    <div className={rootClassName} {...rootAriaProps}>
      {isModal && <div className="mission-overlay__backdrop" />}
      <div className="mission-overlay__card">
        {showActive && (
          <MissionActiveView
            onAbort={handleAbort}
            elapsed={elapsed}
            abortError={abortError}
          />
        )}
        {showPendingPlanApproval && (
          <MissionPlanPreview mission={mission} elapsed={elapsed} />
        )}
        {showCompletePending && (
          <MissionReviewOutcome
            mission={mission}
            elapsed={elapsed}
            onResolved={() => {}}
          />
        )}
        {showTerminal && (
          <MissionTerminalOutcome
            mission={mission}
            elapsed={elapsed}
            onDone={dismissTerminalOverlay}
            onReverted={(outcome) =>
              applyRevertOutcome(mission.id, outcome)
            }
            onViewChanges={() => {
              openMission(mission.id, null);
              dismissTerminalOverlay();
            }}
          />
        )}
      </div>
    </div>
  );
}
