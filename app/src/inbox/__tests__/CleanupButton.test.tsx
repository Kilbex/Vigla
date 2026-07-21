import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import CleanupButton from "../CleanupButton";

vi.mock("../../bindings", () => ({
  commands: {
    cleanupMissionArtifacts: vi.fn(),
  },
}));

import { commands } from "../../bindings";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("CleanupButton", () => {
  it("explains scope and waits for explicit confirmation", () => {
    render(<CleanupButton missionId="mission-1" />);
    fireEvent.click(
      screen.getByRole("button", { name: /clean up mission artifacts/i }),
    );
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByText(/target branch and its commits are unchanged/i)).toBeTruthy();
    expect(commands.cleanupMissionArtifacts).not.toHaveBeenCalled();
  });

  it("calls cleanup and replaces the action with a completion status", async () => {
    (commands.cleanupMissionArtifacts as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "ok",
      data: null,
    });
    const onCleaned = vi.fn();
    render(<CleanupButton missionId="mission-1" onCleaned={onCleaned} />);
    fireEvent.click(
      screen.getByRole("button", { name: /clean up mission artifacts/i }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: /confirm artifact cleanup/i }),
    );

    expect(commands.cleanupMissionArtifacts).toHaveBeenCalledWith("mission-1");
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Artifacts cleaned.",
    );
    expect(onCleaned).toHaveBeenCalledOnce();
  });

  it("surfaces a command error and keeps the dialog open", async () => {
    (commands.cleanupMissionArtifacts as ReturnType<typeof vi.fn>).mockResolvedValue({
      status: "error",
      error: "cleanup failed",
    });
    render(<CleanupButton missionId="mission-1" />);
    fireEvent.click(
      screen.getByRole("button", { name: /clean up mission artifacts/i }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: /confirm artifact cleanup/i }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("cleanup failed");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });
});
