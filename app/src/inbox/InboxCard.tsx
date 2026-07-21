// Single inbox-card visual. Pure render — all state lives in the
// missions store. Acknowledging a card dismisses that notification;
// mission dispositions remain explicit in the mission review surface.

import { useCallback } from "react";
import type { InboxCard } from "./types";
import { boundLabel, kindLabel, severityGlyph } from "./types";

interface InboxCardViewProps {
  card: InboxCard;
  /** Called when the user acknowledges an active Escalation card.
   *  The internal `resolve` action marks only this notification as read. */
  onResolve: (id: string) => void;
}

export default function InboxCardView({ card, onResolve }: InboxCardViewProps) {
  const handleResolve = useCallback(() => onResolve(card.id), [card.id, onResolve]);

  const canResolve = card.kind === "escalation" && !card.resolved;
  const boundText = boundLabel(card.bound);

  return (
    <article
      className={[
        "inbox-card",
        `inbox-card--${card.kind}`,
        `inbox-card--severity-${card.severity}`,
        card.resolved ? "inbox-card--resolved" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      aria-label={`${kindLabel(card.kind)} card`}
    >
      <header className="inbox-card-row">
        <span className="inbox-card-glyph" aria-hidden="true">
          {severityGlyph(card.severity)}
        </span>
        <span className="inbox-card-kind">{kindLabel(card.kind)}</span>
        {boundText ? (
          <span className="inbox-card-bound">{boundText}</span>
        ) : null}
        <h3 className="inbox-card-title">{card.title}</h3>
      </header>
      {card.detail ? (
        <p className="inbox-card-detail">{card.detail}</p>
      ) : null}
      {canResolve ? (
        <footer className="inbox-card-actions">
          <button
            type="button"
            className="inbox-card-resolve"
            onClick={handleResolve}
          >
            Acknowledge
          </button>
        </footer>
      ) : null}
    </article>
  );
}
