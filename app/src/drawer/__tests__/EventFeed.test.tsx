import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Event } from "../../bindings";
import EventFeed from "../EventFeed";

function event(seq: number): Event {
  return {
    schema_version: "1.0",
    worker_id: "worker-1",
    task_id: null,
    seq,
    ts: "2026-07-21T12:00:00Z",
    type: "state_change",
    payload: { state: "executing", from: "idle", note: null },
  };
}

describe("EventFeed", () => {
  it("keeps the scroll region keyboard-accessible and bounds rendered rows", () => {
    const events = Array.from({ length: 501 }, (_, index) => event(index + 1));
    const { container } = render(<EventFeed events={events} />);

    const log = screen.getByRole("log", { name: /worker event log/i });
    expect(log.getAttribute("tabindex")).toBe("0");
    expect(container.querySelectorAll(".drawer-feed-line")).toHaveLength(500);
    expect(screen.getByRole("note")).toHaveTextContent(
      "Showing the newest 500 of 501 matching events.",
    );
    expect(screen.queryByText("#1")).toBeNull();
    expect(screen.getByText("#501")).toBeTruthy();
  });

  it("follows and announces when a full render window rotates at the same count", () => {
    vi.useFakeTimers();
    const scrollSpy = vi.spyOn(
      window.HTMLElement.prototype,
      "scrollIntoView",
    );

    try {
      const { rerender } = render(
        <EventFeed
          events={Array.from({ length: 500 }, (_, index) => event(index + 1))}
        />,
      );

      expect(scrollSpy).toHaveBeenCalledTimes(1);
      act(() => vi.advanceTimersByTime(500));

      rerender(
        <EventFeed
          events={Array.from({ length: 500 }, (_, index) => event(index + 2))}
        />,
      );
      act(() => vi.advanceTimersByTime(500));

      expect(scrollSpy).toHaveBeenCalledTimes(2);
      expect(
        document.querySelector('[aria-live="polite"]'),
      ).toHaveTextContent("Latest event #501.");
    } finally {
      scrollSpy.mockRestore();
      vi.useRealTimers();
    }
  });
});
