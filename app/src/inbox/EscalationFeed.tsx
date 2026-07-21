// S3 — list of currently-active escalations (unresolved Escalation
// cards). Renders inside InboxOverview by default; can be embedded
// elsewhere if needed.

import InboxCardView from "./InboxCard";
import type { InboxCard } from "./types";

interface EscalationFeedProps {
  cards: InboxCard[];
  onResolve: (id: string) => void;
}

export default function EscalationFeed({ cards, onResolve }: EscalationFeedProps) {
  const active = cards.filter(
    (c) => c.kind === "escalation" && !c.resolved,
  );

  if (active.length === 0) {
    return (
      <section className="escalation-feed escalation-feed--empty" aria-label="Escalations">
        <div className="escalation-feed-empty">no active escalations</div>
      </section>
    );
  }

  return (
    <section className="escalation-feed" aria-label="Escalations">
      {active.map((card) => (
        <InboxCardView key={card.id} card={card} onResolve={onResolve} />
      ))}
    </section>
  );
}
