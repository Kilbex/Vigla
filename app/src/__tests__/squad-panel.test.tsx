import { describe, it, expect, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import SquadPanel from "../comms/SquadPanel";
import { useOpsStore } from "../store";
import { initialReplayState } from "../replay/state";
import { emptyState } from "../store/ingest";
import { SQUAD_COLORS } from "../store/types";

beforeEach(() => {
  useOpsStore.setState({
    ...emptyState(),
    replay: initialReplayState,
    liveSnapshot: null,
  });
});

describe("SquadPanel — color radiogroup keyboard navigation (audit-r5)", () => {
  it("only the selected swatch is in the tab order", () => {
    render(<SquadPanel />);
    fireEvent.click(screen.getByRole("button", { name: /create squad/i }));

    const swatches = screen.getAllByRole("radio");
    expect(swatches).toHaveLength(SQUAD_COLORS.length);

    const tabbable = swatches.filter((s) => s.tabIndex === 0);
    expect(tabbable).toHaveLength(1);
    // The default selection is "indigo" (first in SQUAD_COLORS).
    expect(tabbable[0]).toHaveAttribute("aria-label", "indigo");

    // All other swatches are explicitly tabIndex=-1.
    swatches
      .filter((s) => s.getAttribute("aria-label") !== "indigo")
      .forEach((s) => expect(s.tabIndex).toBe(-1));
  });

  it("ArrowRight advances focus + selection to the next swatch and wraps at the end", () => {
    render(<SquadPanel />);
    fireEvent.click(screen.getByRole("button", { name: /create squad/i }));
    const swatches = screen.getAllByRole("radio");

    swatches[0].focus();
    fireEvent.keyDown(swatches[0], { key: "ArrowRight" });
    expect(swatches[1]).toHaveAttribute("aria-checked", "true");
    expect(swatches[1].tabIndex).toBe(0);
    expect(swatches[0].tabIndex).toBe(-1);
    expect(document.activeElement).toBe(swatches[1]);

    // Wrap from the last swatch back to the first.
    swatches[swatches.length - 1].focus();
    fireEvent.keyDown(swatches[swatches.length - 1], { key: "ArrowRight" });
    expect(swatches[0]).toHaveAttribute("aria-checked", "true");
    expect(document.activeElement).toBe(swatches[0]);
  });

  it("ArrowLeft moves focus + selection to the previous swatch and wraps at the start", () => {
    render(<SquadPanel />);
    fireEvent.click(screen.getByRole("button", { name: /create squad/i }));
    const swatches = screen.getAllByRole("radio");

    swatches[0].focus();
    fireEvent.keyDown(swatches[0], { key: "ArrowLeft" });
    expect(swatches[swatches.length - 1]).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(document.activeElement).toBe(swatches[swatches.length - 1]);
  });

  it("ArrowDown / ArrowUp behave like Right / Left for the radiogroup", () => {
    render(<SquadPanel />);
    fireEvent.click(screen.getByRole("button", { name: /create squad/i }));
    const swatches = screen.getAllByRole("radio");

    swatches[0].focus();
    fireEvent.keyDown(swatches[0], { key: "ArrowDown" });
    expect(swatches[1]).toHaveAttribute("aria-checked", "true");

    fireEvent.keyDown(swatches[1], { key: "ArrowUp" });
    expect(swatches[0]).toHaveAttribute("aria-checked", "true");
  });

  it("clicking a swatch still selects it (mouse path unchanged)", () => {
    render(<SquadPanel />);
    fireEvent.click(screen.getByRole("button", { name: /create squad/i }));
    const swatches = screen.getAllByRole("radio");

    fireEvent.click(swatches[2]);
    expect(swatches[2]).toHaveAttribute("aria-checked", "true");
    expect(swatches[2].tabIndex).toBe(0);
  });
});
