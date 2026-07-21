// S10 — RevertButton component test. Asserts that the button
// renders, opens a confirmation dialog on click, surfaces the
// rollback anchor in the dialog body, and only invokes the
// revertMission command after the user confirms.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import RevertButton from "../RevertButton";

vi.mock("../../bindings", () => ({
  commands: {
    revertMission: vi.fn(),
  },
}));

import { commands } from "../../bindings";

beforeEach(() => {
  vi.clearAllMocks();
});

const PROPS = {
  missionId: "mission-1",
  rollbackAnchor: "vigla/revert/mission-1/before/main",
};

describe("RevertButton", () => {
  it("renders, opens dialog on click, and surfaces the rollback anchor", () => {
    render(<RevertButton {...PROPS} />);
    const btn = screen.getByRole("button", { name: /revert mission/i });
    expect(btn).toBeTruthy();
    fireEvent.click(btn);
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByText(/vigla\/revert\/mission-1\/before\/main/)).toBeTruthy();
    expect(commands.revertMission).not.toHaveBeenCalled();
  });

  it("calls revertMission with the typed payload on confirm", () => {
    (commands.revertMission as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: {
        restored_sha: "abc1234",
        pre_merge_tag: "vigla/pre-merge/mission-1",
      },
    });
    render(<RevertButton {...PROPS} />);
    fireEvent.click(screen.getByRole("button", { name: /revert mission/i }));
    fireEvent.click(screen.getByRole("button", { name: /confirm revert/i }));
    expect(commands.revertMission).toHaveBeenCalledWith("mission-1");
  });

  it("closes the dialog on cancel without invoking the command", () => {
    render(<RevertButton {...PROPS} />);
    fireEvent.click(screen.getByRole("button", { name: /revert mission/i }));
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(commands.revertMission).not.toHaveBeenCalled();
  });

  it("surfaces a friendly error if revertMission returns Err", async () => {
    (commands.revertMission as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "error",
      error: "cannot revert paused mission",
    });
    render(<RevertButton {...PROPS} />);
    fireEvent.click(screen.getByRole("button", { name: /revert mission/i }));
    fireEvent.click(screen.getByRole("button", { name: /confirm revert/i }));
    expect(
      await screen.findByText(/cannot revert paused mission/i),
    ).toBeTruthy();
  });

  it("disables the button when `disabled` prop is true", () => {
    render(<RevertButton {...PROPS} disabled />);
    expect(
      (screen.getByRole("button", { name: /revert mission/i }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });
});
