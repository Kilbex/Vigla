import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { beforeEach, describe, it, expect, vi } from "vitest";

vi.mock("../../bindings", () => ({
  commands: {
    rejectPlan: vi.fn(),
  },
}));

import { commands } from "../../bindings";
import PlanRejectForm from "../PlanRejectForm";

describe("PlanRejectForm", () => {
  beforeEach(() => {
    vi.mocked(commands.rejectPlan).mockReset();
  });

  it("calls rejectPlan with the trimmed reason when Confirm reject is clicked", async () => {
    vi.mocked(commands.rejectPlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    const onClose = vi.fn();
    render(<PlanRejectForm generation={3} onClose={onClose} />);

    fireEvent.change(screen.getByLabelText(/reject reason/i), {
      target: { value: "  scope too broad  " },
    });
    fireEvent.click(screen.getByRole("button", { name: /^confirm reject$/i }));

    await waitFor(() => {
      expect(commands.rejectPlan).toHaveBeenCalledWith(3, "scope too broad");
    });
    await waitFor(() => {
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  });

  it("calls rejectPlan with null when no reason is given", async () => {
    vi.mocked(commands.rejectPlan).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });
    const onClose = vi.fn();
    render(<PlanRejectForm generation={3} onClose={onClose} />);

    fireEvent.click(
      screen.getByRole("button", { name: /confirm reject without reason/i }),
    );

    await waitFor(() => {
      expect(commands.rejectPlan).toHaveBeenCalledWith(3, null);
    });
  });

  it("surfaces an inline error and stays open when rejectPlan returns error status", async () => {
    vi.mocked(commands.rejectPlan).mockResolvedValueOnce({
      status: "error",
      error: "wrong state",
    });
    const onClose = vi.fn();
    render(<PlanRejectForm generation={3} onClose={onClose} />);

    fireEvent.click(
      screen.getByRole("button", { name: /confirm reject without reason/i }),
    );

    await waitFor(() => {
      expect(screen.getByText(/wrong state/i)).toBeInTheDocument();
    });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("disables Confirm reject (with reason) when textarea is empty", () => {
    render(<PlanRejectForm generation={3} onClose={() => {}} />);
    const btn = screen.getByRole("button", {
      name: /^confirm reject$/i,
    }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("re-enables the buttons after an inline error so the user can retry", async () => {
    vi.mocked(commands.rejectPlan).mockResolvedValueOnce({
      status: "error",
      error: "wrong state",
    });
    render(<PlanRejectForm generation={3} onClose={() => {}} />);

    const noReason = screen.getByRole("button", {
      name: /confirm reject without reason/i,
    }) as HTMLButtonElement;
    fireEvent.click(noReason);
    await waitFor(() => {
      expect(screen.getByText(/wrong state/i)).toBeInTheDocument();
    });
    expect(noReason.disabled).toBe(false);
  });
});
