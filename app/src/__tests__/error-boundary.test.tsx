import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import ErrorBoundary from "../ErrorBoundary";

function Crasher({ crash }: { crash: boolean }) {
  if (crash) throw new Error("deterministic crash");
  return <div>healthy surface</div>;
}

describe("ErrorBoundary", () => {
  it("resets a captured error when resetKey changes", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { rerender } = render(
      <ErrorBoundary label="Right rail" resetKey="history">
        <Crasher crash />
      </ErrorBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(/deterministic crash/i);

    rerender(
      <ErrorBoundary label="Right rail" resetKey="inbox">
        <Crasher crash={false} />
      </ErrorBoundary>,
    );

    expect(screen.getByText("healthy surface")).toBeTruthy();
    vi.restoreAllMocks();
  });

  it("retry remounts the current child when the payload is fixed", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let crash = true;
    const { rerender } = render(
      <ErrorBoundary label="Right rail" resetKey="history">
        <Crasher crash={crash} />
      </ErrorBoundary>,
    );

    crash = false;
    rerender(
      <ErrorBoundary label="Right rail" resetKey="history">
        <Crasher crash={crash} />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: /retry surface/i }));

    expect(screen.getByText("healthy surface")).toBeTruthy();
    vi.restoreAllMocks();
  });
});
