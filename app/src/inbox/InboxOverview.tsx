// Top-level inbox surface. Replaces CommsFeed as the
// default right-rail when the "Show all events" preference is
// off. Composes the EscalationFeed plus a short list of recent
// Completion cards so the user has both "what needs you" and
// "what shipped" on one screen.
// Cross-mission audit history lives on the dedicated History route;
// this view intentionally stays scoped to the active mission.

import { useCallback } from "react";
import ReviewQueue from "../comms/ReviewQueue";
import { selectWorkersNeedingReview, useOpsStore } from "../store";
import { applyInboxAction } from "./InboxState";
import EscalationFeed from "./EscalationFeed";
import InboxCardView from "./InboxCard";
import {
  selectActiveMission,
  useMissionsStore,
} from "../missions/store";
import type { InboxCard } from "./types";
import { requiresAction } from "./types";

const COMPLETION_LIMIT = 5;

export default function InboxOverview() {
  const mission = useMissionsStore(selectActiveMission);
  const setInboxFor = useMissionsStore((s) => s.setInboxForActive);
  const workerNeedsInput = useOpsStore((s) => selectWorkersNeedingReview(s).length);

  const handleResolve = useCallback(
    (id: string) => {
      if (!mission) return;
      const after = applyInboxAction(
        { cards: mission.inbox },
        { type: "resolve", id },
      );
      setInboxFor(mission.id, after.cards);
    },
    [mission, setInboxFor],
  );

  if (!mission) {
    return (
      <aside className="inbox-overview inbox-overview--empty" aria-label="Inbox">
        <header className="inbox-overview-header">
          <h2 className="inbox-overview-heading" aria-label="Inbox">Inbox</h2>
        </header>
        <div className="inbox-overview-empty">
          <span className="inbox-overview-empty__glyph" aria-hidden="true">&#x2205;</span>
          <div className="inbox-overview-empty__label">No active mission</div>
        </div>
      </aside>
    );
  }

  const inbox = mission.inbox;
  const awaitingDecision =
    mission.lifecycle === "complete_pending_merge" ||
    mission.lifecycle === "attention";
  const decisionCard: InboxCard | null = awaitingDecision
    ? {
        id: `${mission.id}:decision`,
        missionId: mission.id,
        seq: Number.MAX_SAFE_INTEGER,
        surfacedAt: mission.updatedAt,
        kind: "completion",
        severity: "action_required",
        title:
          mission.lifecycle === "attention"
            ? "Mission paused — decision required"
            : "Mission complete — awaiting your decision",
        detail: mission.completionSummary ?? mission.statusLine,
        bound: null,
        resolved: false,
      }
    : null;
  const attentionCards = [
    ...(decisionCard ? [decisionCard] : []),
    ...inbox.filter(requiresAction),
  ];
  const completions = inbox
    .filter(
      (c: InboxCard) =>
        c.kind === "completion" &&
        !requiresAction(c) &&
        !(awaitingDecision && /mission complete/i.test(c.title)),
    )
    .slice(-COMPLETION_LIMIT)
    .reverse();
  const sideEffects = inbox.filter((c: InboxCard) => c.kind === "side_effect");

  return (
    <aside className="inbox-overview" aria-label="Inbox">
      <header className="inbox-overview-header">
        <h2 className="inbox-overview-heading" aria-label="Inbox">Inbox</h2>
      </header>

      <section className="inbox-section" aria-label="Needs attention">
        <h3 className="inbox-section-title" aria-label="Needs attention">Needs attention</h3>
        {attentionCards.length > 0 ? (
          attentionCards.map((c) => (
            <InboxCardView key={c.id} card={c} onResolve={handleResolve} />
          ))
        ) : workerNeedsInput === 0 ? (
          <EscalationFeed cards={inbox} onResolve={handleResolve} />
        ) : null}
        {mission.lifecycle !== "merged" &&
        mission.lifecycle !== "discarded" &&
        mission.lifecycle !== "aborted" &&
        mission.lifecycle !== "extended" ? (
          <ReviewQueue />
        ) : null}
      </section>

      {sideEffects.length > 0 ? (
        <section className="inbox-section" aria-label="Side effects">
          <h3 className="inbox-section-title" aria-label="Side effects">Side effects</h3>
          {sideEffects.map((c) => (
            <InboxCardView key={c.id} card={c} onResolve={handleResolve} />
          ))}
        </section>
      ) : null}

      <section className="inbox-section" aria-label="Recent completions">
        <h3 className="inbox-section-title" aria-label="Recent completions">Recent completions</h3>
        {completions.length === 0 ? (
          <div className="inbox-section-empty">no completions yet</div>
        ) : (
          completions.map((c) => (
            <InboxCardView key={c.id} card={c} onResolve={handleResolve} />
          ))
        )}
      </section>
    </aside>
  );
}
