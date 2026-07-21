import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../bindings", () => ({
  commands: {
    appSettings: vi.fn(),
    startMockWorker: vi.fn(),
    startMission: vi.fn(),
    openCliLogin: vi.fn(),
    openCliAuthDocs: vi.fn(),
  },
}));

import { commands } from "../bindings";
import Settings from "../settings/Settings";

const goodResponse = {
  version: "0.0.1",
  db_path: "/path/to/db",
  configured_repo_root: "/tmp/vigla-l1-quota/repo",
  mock_harness_path: "/path/to/mock",
  mock_harness_present: true,
  l1_quota_mock_enabled: false,
  claude_present: true,
  codex_present: false,
  gemini_present: true,
  antigravity_present: false,
  kiro_present: false,
  copilot_present: false,
  cli_auth: [
    {
      vendor: "claude",
      display_name: "Claude CLI",
      binary: "claude",
      binary_present: true,
      state: "ready",
      detail: "Authenticated according to `claude auth status`.",
      login_command: "claude auth login",
      docs_url: "https://code.claude.com/docs/en/authentication",
    },
    {
      vendor: "codex",
      display_name: "Codex CLI",
      binary: "codex",
      binary_present: true,
      state: "not_logged_in",
      detail: "Run `codex login` to authenticate this CLI.",
      login_command: "codex login",
      docs_url: "https://github.com/openai/codex",
    },
    {
      vendor: "gemini",
      display_name: "Gemini CLI (legacy)",
      binary: "gemini",
      binary_present: false,
      state: "missing_cli",
      detail: "gemini is not available on PATH.",
      login_command: "gemini",
      docs_url:
        "https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals",
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe("Settings — IPC error UX (audit r5 polish)", () => {
  it("renders the body when appSettings resolves", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    render(<Settings open onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument();
    });
    expect(screen.getByText(/\/path\/to\/db/)).toBeInTheDocument();
    expect(screen.getByText("Antigravity CLI")).toBeInTheDocument();
    expect(screen.getByText("Kiro CLI")).toBeInTheDocument();
    expect(screen.getByText("GitHub Copilot CLI")).toBeInTheDocument();
    expect(screen.getAllByText("Gemini CLI (legacy)").length).toBeGreaterThan(0);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("renders only an error (not the stale body) when appSettings rejects", async () => {
    (commands.appSettings as any).mockRejectedValue(
      new Error("orchestrator unreachable"),
    );
    render(<Settings open onClose={() => {}} />);
    await waitFor(() => {
      expect(screen.getByText(/orchestrator unreachable/i)).toBeInTheDocument();
    });
    // No stale data body — the placeholder "loading…" hides until
    // either branch resolves; rejection should not surface stale data.
    expect(screen.queryByText(/v0\.0\.1/)).toBeNull();
    expect(screen.queryByText(/\/path\/to\/db/)).toBeNull();
  });

  it("clears the error when a subsequent open succeeds (no contradictory state)", async () => {
    // First open: rejects → error visible.
    (commands.appSettings as any).mockRejectedValueOnce(
      new Error("first attempt failed"),
    );
    const { rerender } = render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/first attempt failed/i)).toBeInTheDocument(),
    );

    // Close, then reopen with a successful mock.
    rerender(<Settings open={false} onClose={() => {}} />);
    (commands.appSettings as any).mockResolvedValueOnce(goodResponse);
    rerender(<Settings open onClose={() => {}} />);

    await waitFor(() => {
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument();
    });
    // Error from the prior attempt must be gone — no contradictory
    // "error banner above stale-or-fresh body" UX.
    expect(screen.queryByText(/first attempt failed/i)).toBeNull();
  });

  it("clears the body when a subsequent open rejects (no stale data behind error)", async () => {
    // First open: succeeds → body visible.
    (commands.appSettings as any).mockResolvedValueOnce(goodResponse);
    const { rerender } = render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument(),
    );

    // Close, then reopen with a rejecting mock.
    rerender(<Settings open={false} onClose={() => {}} />);
    (commands.appSettings as any).mockRejectedValueOnce(
      new Error("second attempt failed"),
    );
    rerender(<Settings open onClose={() => {}} />);

    await waitFor(() =>
      expect(screen.getByText(/second attempt failed/i)).toBeInTheDocument(),
    );
    // Stale body from the prior success must be gone — the audit-r5
    // bug was rendering both the error banner AND the prior body
    // simultaneously, contradicting each other on screen.
    expect(screen.queryByText(/v0\.0\.1/)).toBeNull();
  });
});

describe("Settings — Developer section (Step 23)", () => {
  it("renders mock spawn buttons + reset under DEVELOPER", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/^Developer$/)).toBeInTheDocument(),
    );
    expect(
      screen.getByRole("button", { name: /^claude_happy$/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^codex_blocked$/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^gemini_happy$/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /reset operations room/i }),
    ).toBeInTheDocument();
  });

  it("dispatches startMockWorker when a mock spawn button is clicked", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    (commands.startMockWorker as any).mockResolvedValue({
      status: "ok",
      data: "mock-id-1",
    });
    render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /^claude_happy$/i }),
      ).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: /^claude_happy$/i }));
    await waitFor(() =>
      expect(commands.startMockWorker).toHaveBeenCalledWith(
        "claude_happy",
        1.0,
      ),
    );
    await waitFor(() =>
      expect(screen.getByText(/started claude_happy/i)).toBeInTheDocument(),
    );
  });

  it("starts the L1 quota mission when the gated dev action is enabled", async () => {
    (commands.appSettings as any).mockResolvedValue({
      ...goodResponse,
      l1_quota_mock_enabled: true,
    });
    (commands.startMission as any).mockResolvedValue({
      status: "ok",
      data: "mission-l1-quota",
    });
    render(<Settings open onClose={() => {}} />);
    const start = await screen.findByRole("button", {
      name: /start quota mission/i,
    });

    fireEvent.click(start);

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledWith(
        expect.objectContaining({
          supervisor_model: "claude",
          worker_model: "claude_quota_exhausted",
          worker_count: 1,
        }),
        "/tmp/vigla-l1-quota/repo",
      );
    });
    expect(screen.getByText(/started L1 quota mission/i)).toBeInTheDocument();
  });
});

describe("Settings — CLI auth", () => {
  it("renders login state for each real CLI", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/CLI auth/i)).toBeInTheDocument(),
    );

    expect(screen.getAllByText("Claude CLI").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Codex CLI").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Gemini CLI (legacy)").length).toBeGreaterThan(0);
    expect(screen.getByText(/Logged in/i)).toBeInTheDocument();
    expect(screen.getByText(/Login needed/i)).toBeInTheDocument();
    expect(screen.getByText(/CLI missing/i)).toBeInTheDocument();
  });

  it("opens a terminal login flow for a logged-out CLI", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    (commands.openCliLogin as any).mockResolvedValue({
      status: "ok",
      data: null,
    });
    render(<Settings open onClose={() => {}} />);
    const login = await screen.findByRole("button", {
      name: /log in to codex cli/i,
    });

    fireEvent.click(login);

    await waitFor(() =>
      expect(commands.openCliLogin).toHaveBeenCalledWith("codex"),
    );
    await waitFor(() =>
      expect(screen.getByText(/opened codex login/i)).toBeInTheDocument(),
    );
  });

  it("opens vendor auth docs from the CLI auth section", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    (commands.openCliAuthDocs as any).mockResolvedValue({
      status: "ok",
      data: null,
    });
    render(<Settings open onClose={() => {}} />);
    const docs = await screen.findByRole("button", {
      name: /open claude cli auth docs/i,
    });

    fireEvent.click(docs);

    await waitFor(() =>
      expect(commands.openCliAuthDocs).toHaveBeenCalledWith("claude"),
    );
  });
});

describe("Settings — section chrome (P2-16)", () => {
  it("renders an Environment section header above the env rows", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument(),
    );
    const header = screen.getByText("Environment");
    expect(header).toBeInTheDocument();
    expect(header.tagName).toBe("H3");
    expect(header.classList.contains("settings-section-title")).toBe(true);
  });

  it("renders exactly five top-level section titles in the expected order", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    const { container } = render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument(),
    );
    const titles = Array.from(
      container.querySelectorAll(".settings-section-title"),
    ).map((el) => el.textContent);
    expect(titles).toEqual([
      "Environment",
      "CLI auth",
      "Shortcuts",
      "Preferences",
      "Developer",
    ]);
  });

  it("keeps the env rows inside the Environment section", async () => {
    (commands.appSettings as any).mockResolvedValue(goodResponse);
    render(<Settings open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/v0\.0\.1/)).toBeInTheDocument(),
    );
    const envHeader = screen.getByText("Environment");
    const envSection = envHeader.closest("section");
    expect(envSection).not.toBeNull();
    // App version, Database, and Mock harness rows must live under
    // the same <section> as the Environment header — otherwise the
    // refactor failed to scope these rows to the new group.
    const labels = Array.from(
      envSection!.querySelectorAll(".settings-row-label"),
    ).map((el) => el.textContent);
    expect(labels).toEqual(
      expect.arrayContaining(["App version", "Database", "Mock harness"]),
    );
    // Sanity — shortcut rows must NOT be inside the Environment section.
    expect(labels).not.toContain("⌘ 1");
  });
});
