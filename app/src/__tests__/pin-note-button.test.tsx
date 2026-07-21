import { describe, it, expect, beforeEach, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useMissionsStore } from "../missions/store";
import PinNoteButton from "../memory/PinNoteButton";

// Mock the tauri-specta bindings. Each test reconfigures
// `memoryPinNote` to return the relevant outcome.
const memoryPinNote = vi.fn();

vi.mock("../bindings", () => {
  return {
    commands: {
      memoryPinNote: (...args: unknown[]) => memoryPinNote(...args),
    },
  };
});

const REPO_CWD = "/tmp/test-repo";

describe("PinNoteButton", () => {
  beforeEach(() => {
    memoryPinNote.mockReset();
    // A2: seed the current repo cwd so the button is enabled. The
    // disabled-when-null behaviour is covered in its own test below.
    useMissionsStore.getState().setCurrentRepoCwd(REPO_CWD);
  });

  it("renders the trigger button collapsed", () => {
    render(<PinNoteButton />);
    expect(screen.getByRole("button", { name: /pin note/i })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens the dialog on click and focuses the textarea", () => {
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByLabelText(/note body/i)).toHaveFocus();
  });

  it("disables Pin until body has non-whitespace content", () => {
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    const pin = screen.getByRole("button", { name: /^pin$/i });
    expect(pin).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/note body/i), {
      target: { value: "   \n  " },
    });
    expect(pin).toBeDisabled();
    fireEvent.change(screen.getByLabelText(/note body/i), {
      target: { value: "Always run cargo build." },
    });
    expect(pin).not.toBeDisabled();
  });

  it("submits and shows the learned badge on a successful pin", async () => {
    memoryPinNote.mockResolvedValue({
      status: "ok",
      data: { outcome: "pinned", note_id: "01J", promoted: true },
    });
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    fireEvent.change(screen.getByLabelText(/note body/i), {
      target: { value: "Run tests before merge." },
    });
    fireEvent.click(screen.getByRole("button", { name: /^pin$/i }));
    await waitFor(() => {
      expect(memoryPinNote).toHaveBeenCalledTimes(1);
    });
    expect(memoryPinNote).toHaveBeenCalledWith(REPO_CWD, {
      kind: "fact",
      scope_kind: "repo",
      scope_value: null,
      body: "Run tests before merge.",
    });
    await waitFor(() => {
      expect(screen.getByText(/learned/i)).toBeInTheDocument();
    });
  });

  it("displays the redacted preview on a Secret rejection", async () => {
    memoryPinNote.mockResolvedValue({
      status: "ok",
      data: {
        outcome: "rejected",
        reason: "secret",
        redacted_preview: "Deploy with [REDACTED:20:abcd1234] in env.",
      },
    });
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    fireEvent.change(screen.getByLabelText(/note body/i), {
      target: { value: "Deploy with AKIAIOSFODNN7EXAMPLE in env." },
    });
    fireEvent.click(screen.getByRole("button", { name: /^pin$/i }));
    await waitFor(() => {
      expect(screen.getByText(/Rejected \(secret\)/)).toBeInTheDocument();
    });
    expect(screen.getByText(/\[REDACTED:20/)).toBeInTheDocument();
    // Dialog stays open after rejection so the user can adjust.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("surfaces IPC errors without closing the dialog", async () => {
    memoryPinNote.mockResolvedValue({
      status: "error",
      error: "memory backend error: pool closed",
    });
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    fireEvent.change(screen.getByLabelText(/note body/i), {
      target: { value: "x" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^pin$/i }));
    await waitFor(() => {
      expect(screen.getByText(/pool closed/)).toBeInTheDocument();
    });
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByLabelText(/note body/i)).toHaveValue("x");
  });

  it("selects a different kind when the user picks it", () => {
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    fireEvent.change(screen.getByLabelText(/^kind$/i), {
      target: { value: "hazard" },
    });
    expect(screen.getByLabelText(/^kind$/i)).toHaveValue("hazard");
  });

  it("closes the dialog on Cancel", () => {
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes the dialog on Escape", () => {
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("submits on Cmd+Enter from the textarea", async () => {
    memoryPinNote.mockResolvedValue({
      status: "ok",
      data: { outcome: "pinned", note_id: "01J", promoted: true },
    });
    render(<PinNoteButton />);
    fireEvent.click(screen.getByRole("button", { name: /pin note/i }));
    const textarea = screen.getByLabelText(/note body/i);
    fireEvent.change(textarea, { target: { value: "x" } });
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    await waitFor(() => {
      expect(memoryPinNote).toHaveBeenCalledTimes(1);
    });
  });

  // A2: with no active repo, the button is disabled and never
  // attempts an IPC call. Users see a helpful tooltip.
  it("is disabled when no mission has been started this session", () => {
    useMissionsStore.getState().reset(); // clears currentRepoCwd
    render(<PinNoteButton />);
    const btn = screen.getByRole("button", { name: /pin note/i });
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute(
      "title",
      expect.stringMatching(/start a mission first/i),
    );
    fireEvent.click(btn);
    // No dialog opens — clicks on disabled buttons don't fire onClick
    // in this jsdom config; assertion is the absence of side effects.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(memoryPinNote).not.toHaveBeenCalled();
  });
});
