import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import EscalationFeed from "../inbox/EscalationFeed";
import type { InboxCard } from "../inbox/types";

function card(overrides: Partial<InboxCard> = {}): InboxCard {
  return {
    id: "m1:1:w-1",
    missionId: "m1",
    seq: 1,
    surfacedAt: "2026-05-21T00:00:00.000Z",
    kind: "escalation",
    severity: "action_required",
    title: "Quality bound tripped",
    detail: "audit overall 0.4 < 0.7",
    bound: "quality",
    resolved: false,
    ...overrides,
  };
}

describe("EscalationFeed", () => {
  it("renders zero cards when inbox is empty", () => {
    render(<EscalationFeed cards={[]} onResolve={() => {}} />);
    expect(screen.getByText(/no active escalations/i)).toBeInTheDocument();
  });

  it("renders only unresolved Escalation cards", () => {
    const cards = [
      card({ id: "a", title: "active" }),
      card({ id: "b", title: "resolved", resolved: true }),
      card({
        id: "c",
        kind: "completion",
        severity: "info",
        title: "completion",
      }),
    ];
    render(<EscalationFeed cards={cards} onResolve={() => {}} />);
    expect(screen.getByText("active")).toBeInTheDocument();
    expect(screen.queryByText("resolved")).not.toBeInTheDocument();
    expect(screen.queryByText("completion")).not.toBeInTheDocument();
  });

  it("acknowledges the notification without implying a mission disposition", () => {
    const onResolve = vi.fn();
    render(<EscalationFeed cards={[card()]} onResolve={onResolve} />);
    fireEvent.click(screen.getByRole("button", { name: /acknowledge/i }));
    expect(onResolve).toHaveBeenCalledWith("m1:1:w-1");
  });
});
