import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { MissionEvent } from "../bindings";
import DeployPanel, { parseScopePaths } from "../comms/DeployPanel";
import { useMissionsStore } from "../missions/store";

vi.mock("../bindings", async () => {
  const actual =
    await vi.importActual<typeof import("../bindings")>("../bindings");
  return {
    ...actual,
    commands: {
      startMission: vi.fn(),
      abortMission: vi.fn(),
      resolveMission: vi.fn(),
      checkCliAuth: vi.fn(),
      openCliLogin: vi.fn(),
    },
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import { commands } from "../bindings";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

const MID = "demo-7a3f";

function startedEvent(): MissionEvent {
  return {
    mission_id: MID,
    seq: 0,
    ts: "2026-05-12T00:00:00.000Z",
    type: "mission.created",
    payload: {
      spec: {
        title: "Add logout",
        objective: "Add /api/logout",
        target_ref: "main",
        tests: null,
        supervisor_model: null,
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
    },
  };
}

describe("DeployPanel (unified team-launch surface)", () => {
  beforeEach(() => {
    useMissionsStore.getState().reset();
    vi.clearAllMocks();
    // Sticky-prefs storage must be reset between tests so one test's
    // selection doesn't bleed into the next test's defaults.
    window.localStorage.clear();
    vi.mocked(commands.checkCliAuth).mockResolvedValue([
      {
        vendor: "claude",
        display_name: "Claude CLI",
        binary: "claude",
        binary_present: true,
        state: "ready",
        detail: "Claude credentials were detected locally.",
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
    ]);
  });

  it("shows the unified form with objective, folder, and Start by default", () => {
    render(<DeployPanel />);
    expect(screen.getByText(/DEPLOY WORKERS/i)).toBeTruthy();
    expect(screen.getByLabelText(/mission objective/i)).toBeTruthy();
    expect(
      screen.getByRole("button", { name: /browse for project folder/i }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /start/i })).toBeTruthy();
    expect(screen.getByText(/no folder selected/i)).toBeTruthy();
  });

  it("keeps the Advanced disclosure collapsed by default", () => {
    render(<DeployPanel />);
    const details = screen
      .getByText(/^advanced$/i)
      .closest("details") as HTMLDetailsElement | null;
    expect(details).not.toBeNull();
    expect(details?.open).toBe(false);
  });

  it("Advanced body is bounded so the objective stays visible", () => {
    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    // The bounded-scroll class is the load-bearing piece — the rule
    // itself lives in index.css and is exercised by the e2e visual
    // smoke. Pinning the class here catches accidental rename.
    const body = document.querySelector(".deploy-advanced-body");
    expect(body).toBeTruthy();
  });

  it("hides supervisor / worker / count controls until Advanced is opened", () => {
    render(<DeployPanel />);
    // The selects render in the DOM (inside the closed <details>) but
    // are visually collapsed. The expectation here is that the user
    // does not have to interact with them by default — i.e. Start is
    // enabled with only the two required fields, before Advanced is
    // ever opened.
    const details = screen
      .getByText(/^advanced$/i)
      .closest("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
  });

  it("Advanced exposes supervisor / count selectors and per-worker CLI rows", async () => {
    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));

    const supervisor = screen.getByLabelText(
      /supervisor model/i,
    ) as HTMLSelectElement;
    const count = screen.getByLabelText(
      /number of workers/i,
    ) as HTMLSelectElement;

    expect(supervisor.value).toBe("claude");
    expect(count.value).toBe("auto");

    expect(Array.from(supervisor.options).map((o) => o.value)).toEqual([
      "claude",
    ]);
    expect(supervisor.disabled).toBe(true);
    expect(
      screen.getByText(/additional supervisor providers are roadmap work/i),
    ).toBeTruthy();
    expect(Array.from(count.options).map((o) => o.value)).toEqual([
      "auto",
      "1",
      "2",
      "3",
      "4",
      "5",
    ]);
    expect(screen.getByText(/supervisor will choose worker count/i)).toBeTruthy();

    fireEvent.change(count, { target: { value: "3" } });
    const employee1 = screen.getByLabelText(/employee 1 CLI$/i) as HTMLSelectElement;
    const employee2 = screen.getByLabelText(/employee 2 CLI$/i) as HTMLSelectElement;
    const employee3 = screen.getByLabelText(/employee 3 CLI$/i) as HTMLSelectElement;
    const employee1Model = screen.getByLabelText(
      /employee 1 applied model/i,
    ) as HTMLSelectElement;
    const employee2Model = screen.getByLabelText(
      /employee 2 applied model/i,
    ) as HTMLSelectElement;
    const employee3Model = screen.getByLabelText(
      /employee 3 applied model/i,
    ) as HTMLSelectElement;
    expect(employee1.value).toBe("claude");
    expect(employee2.value).toBe("codex");
    expect(employee3.value).toBe("antigravity");
    expect(employee1Model.value).toBe("");
    expect(employee2Model.value).toBe("");
    expect(employee3Model.value).toBe("");
    expect(Array.from(employee1.options).map((o) => o.value)).toEqual([
      "claude",
      "codex",
      "antigravity",
      "kiro",
      "copilot",
      "gemini",
    ]);
    expect(Array.from(employee2Model.options).map((o) => o.value)).toEqual([
      "",
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.6-luna",
      "gpt-5.5",
      "gpt-5.4",
      "gpt-5.4-mini",
      "gpt-5.3-codex",
      "gpt-5.3-codex-spark",
      "gpt-5.2",
    ]);
  });

  it("shows CLI auth status per selected employee and opens login", async () => {
    vi.mocked(commands.openCliLogin).mockResolvedValueOnce({
      status: "ok",
      data: null,
    });

    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "2" },
    });

    await waitFor(() => {
      expect(screen.getByText(/Logged in/i)).toBeTruthy();
      expect(screen.getByText(/Login needed/i)).toBeTruthy();
    });

    fireEvent.click(screen.getByRole("button", { name: /log in to codex/i }));

    await waitFor(() => {
      expect(commands.openCliLogin).toHaveBeenCalledWith("codex");
    });
  });

  it("disables Start until objective and folder are both set", async () => {
    render(<DeployPanel />);
    const start = screen.getByRole("button", { name: /start/i }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);

    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    expect(start.disabled).toBe(true);

    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => {
      expect(screen.getByText("/tmp/project")).toBeTruthy();
    });
    expect(start.disabled).toBe(false);
  });

  // P0-1: a disabled CTA with no explanation makes first-time users
  // hover repeatedly. The button now publishes a `aria-describedby`
  // hint that names the missing required field(s) so screen-readers
  // and sighted users get the same answer to "why can't I click?".
  it("explains why Start is disabled while required fields are empty", async () => {
    render(<DeployPanel />);

    // Both fields empty → composite reason.
    expect(
      screen.getByTestId("deploy-cta-disabled-reason").textContent,
    ).toMatch(/describe the work and choose a project folder/i);

    // Only objective filled → "choose a folder…".
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    expect(
      screen.getByTestId("deploy-cta-disabled-reason").textContent,
    ).toMatch(/choose a project folder/i);

    // Folder picked → reason disappears, CTA enables.
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => {
      expect(screen.getByText("/tmp/project")).toBeTruthy();
    });
    expect(screen.queryByTestId("deploy-cta-disabled-reason")).toBeNull();

    // The CTA correctly wires the hint via aria-describedby while
    // disabled, and drops the link once the button is enabled. The
    // assertion runs *after* the field-pair becomes ready so we
    // capture the cleanup, not the disabled state.
    const start = screen.getByRole("button", {
      name: /start/i,
    }) as HTMLButtonElement;
    expect(start.getAttribute("aria-describedby")).toBeNull();
  });

  it("describes only the missing folder when objective is the lone empty field", async () => {
    // Pre-seed sticky prefs with a cwd so only the objective is empty
    // on first mount. Mirrors the steady-state case where the same
    // user returns to Vigla and types a new objective.
    window.localStorage.setItem(
      "vigla.deploy.prefs.v1",
      JSON.stringify({ cwd: "/tmp/project" }),
    );
    render(<DeployPanel />);
    expect(
      screen.getByTestId("deploy-cta-disabled-reason").textContent,
    ).toMatch(/describe the work/i);
  });

  it("Start uses defaults (claude supervisor, auto worker, auto count) when Advanced is never opened", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    expect(commands.startMission).toHaveBeenCalledWith(
      {
        title: "Add /api/logout",
        objective: "Add /api/logout",
        target_ref: "",
        tests: null,
        // Claude is the locked-in supervisor default; null worker
        // model/count keeps role routing and supervisor task count.
        supervisor_model: "claude",
        worker_model: null,
        worker_count: null,
        confirm_plan: null,
      },
      "/tmp/project",
    );
  });

  it("does not pin a vendor model when a concrete default roster starts", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Use vendor defaults" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "2" },
    });
    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    expect(vi.mocked(commands.startMission).mock.calls[0][0]).toMatchObject({
      worker_model: "claude,codex",
      worker_count: 2,
    });
  });

  it("Start propagates supported Advanced worker CLI and count overrides", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout\nInvalidate sessions" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "3" },
    });
    fireEvent.change(screen.getByLabelText(/employee 1 CLI$/i), {
      target: { value: "gemini" },
    });
    fireEvent.change(screen.getByLabelText(/employee 2 CLI$/i), {
      target: { value: "codex" },
    });
    fireEvent.change(screen.getByLabelText(/employee 2 applied model/i), {
      target: { value: "gpt-5.6-sol" },
    });
    fireEvent.change(screen.getByLabelText(/employee 3 CLI$/i), {
      target: { value: "claude" },
    });
    fireEvent.change(screen.getByLabelText(/employee 3 applied model/i), {
      target: { value: "fable" },
    });

    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    expect(commands.startMission).toHaveBeenCalledWith(
      {
        title: "Add /api/logout",
        objective: "Add /api/logout\nInvalidate sessions",
        target_ref: "",
        tests: null,
        supervisor_model: "claude",
        worker_model: "gemini:auto,codex:gpt-5.6-sol,claude:fable",
        worker_count: 3,
        confirm_plan: null,
      },
      "/tmp/project",
    );
  });

  it("surfaces a startMission error inline without losing the form state", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "error",
      error: "a mission is already active",
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Try thing" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(screen.getByText(/a mission is already active/i)).toBeTruthy();
    });
    expect(
      (screen.getByLabelText(/mission objective/i) as HTMLTextAreaElement)
        .value,
    ).toBe("Try thing");
    expect(screen.getByText("/tmp/project")).toBeTruthy();
  });

  it("Advanced exposes a Plan-mode radio, default Direct", async () => {
    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    const direct = screen.getByLabelText(
      /direct \(auto-proceed\)/i,
    ) as HTMLInputElement;
    const review = screen.getByLabelText(
      /review \(pause for plan approval\)/i,
    ) as HTMLInputElement;
    expect(direct.checked).toBe(true);
    expect(review.checked).toBe(false);
  });

  it("Plan-mode Review selected → startMission payload has confirm_plan: true", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.click(
      screen.getByLabelText(/review \(pause for plan approval\)/i),
    );
    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    const call = vi.mocked(commands.startMission).mock.calls[0];
    expect(call[0].confirm_plan).toBe(true);
  });

  it("Plan-mode Direct (default) → startMission payload has confirm_plan: null", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    const call = vi.mocked(commands.startMission).mock.calls[0];
    expect(call[0].confirm_plan).toBeNull();
  });

  it("Plan-mode choice persists across mounts (localStorage prefs)", async () => {
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");
    const first = render(<DeployPanel />);
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.click(
      screen.getByLabelText(/review \(pause for plan approval\)/i),
    );
    first.unmount();

    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    const review = screen.getByLabelText(
      /review \(pause for plan approval\)/i,
    ) as HTMLInputElement;
    expect(review.checked).toBe(true);
  });

  it("re-enables Start when startMission rejects (Tauri IPC failure or Rust panic)", async () => {
    // If `commands.startMission` rejects rather than returning
    // {status:"error"}, the original code skipped `setSubmitting(false)`
    // and the form was bricked permanently. The try/catch/finally
    // ensures the submitting flag is cleared on every exit path.
    vi.mocked(commands.startMission).mockRejectedValueOnce(
      new Error("IPC channel closed"),
    );
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Try thing" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    const start = screen.getByRole("button", {
      name: /start/i,
    }) as HTMLButtonElement;
    fireEvent.click(start);

    await waitFor(() => {
      expect(screen.getByText(/IPC channel closed/i)).toBeTruthy();
    });
    // Start button must be re-enabled so the user can retry without
    // reloading the app.
    expect(start.disabled).toBe(false);
    expect(start.textContent?.toLowerCase()).not.toContain("starting");
  });

  it("disables Start and shows an explainer while a mission is already running", () => {
    useMissionsStore.getState().ingest(startedEvent());
    render(<DeployPanel />);
    const start = screen.getByRole("button", { name: /start/i }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    expect(
      screen.getByText(/a mission is already running\. finish or abort/i),
    ).toBeTruthy();
  });

  it("persists cwd + Advanced selections across mounts (localStorage prefs)", async () => {
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");
    const first = render(<DeployPanel />);
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "4" },
    });
    fireEvent.change(screen.getByLabelText(/employee 1 CLI$/i), {
      target: { value: "gemini" },
    });
    fireEvent.change(screen.getByLabelText(/employee 1 applied model/i), {
      target: { value: "flash" },
    });
    fireEvent.change(screen.getByLabelText(/employee 2 CLI$/i), {
      target: { value: "claude" },
    });
    fireEvent.change(screen.getByLabelText(/employee 2 applied model/i), {
      target: { value: "haiku" },
    });

    first.unmount();

    // Mount again — defaults should now come from localStorage.
    render(<DeployPanel />);
    expect(screen.getByText("/tmp/project")).toBeTruthy();
    fireEvent.click(screen.getByText(/^advanced$/i));
    expect(
      (screen.getByLabelText(/supervisor model/i) as HTMLSelectElement).value,
    ).toBe("claude");
    expect(
      (screen.getByLabelText(/number of workers/i) as HTMLSelectElement).value,
    ).toBe("4");
    expect(
      (screen.getByLabelText(/employee 1 CLI$/i) as HTMLSelectElement)
        .value,
    ).toBe("gemini");
    expect(
      (screen.getByLabelText(/employee 1 applied model/i) as HTMLSelectElement)
        .value,
    ).toBe("flash");
    expect(
      (screen.getByLabelText(/employee 2 CLI$/i) as HTMLSelectElement)
        .value,
    ).toBe("claude");
    expect(
      (screen.getByLabelText(/employee 2 applied model/i) as HTMLSelectElement)
        .value,
    ).toBe("haiku");
  });

  it("does NOT persist the objective across mounts (mission-specific)", () => {
    const first = render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Sensitive mission detail" },
    });
    first.unmount();

    render(<DeployPanel />);
    expect(
      (screen.getByLabelText(/mission objective/i) as HTMLTextAreaElement)
        .value,
    ).toBe("");
  });

  it("clears the explainer when the active mission reaches a terminal state", () => {
    useMissionsStore.getState().ingest(startedEvent());
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 1,
      ts: "2026-05-12T00:00:01.000Z",
      type: "mission.completed",
      payload: { summary: "done", files_changed: 0 },
    });
    useMissionsStore.getState().ingest({
      mission_id: MID,
      seq: 2,
      ts: "2026-05-12T00:00:02.000Z",
      type: "mission.merge_resolved",
      payload: { resolution: { type: "merged" } },
    });
    render(<DeployPanel />);
    expect(screen.queryByText(/a mission is already running/i)).toBeNull();
  });

  it("Advanced exposes a scope-paths textarea, empty by default", () => {
    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    const ta = screen.getByLabelText(/scope paths/i) as HTMLTextAreaElement;
    expect(ta.value).toBe("");
  });

  it("omits scope_paths from the IPC payload when the textarea is empty", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: /start/i }));
    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    const [spec] = vi.mocked(commands.startMission).mock.calls[0];
    // The key MUST be absent (not just `undefined`) so the wire shape
    // matches the no-Advanced, no-override baseline that backend test
    // fixtures and snapshot assertions already lock in.
    expect(Object.prototype.hasOwnProperty.call(spec, "scope_paths")).toBe(
      false,
    );
  });

  it("trims, dedupes, and forwards scope_paths when the textarea is non-empty", async () => {
    vi.mocked(commands.startMission).mockResolvedValueOnce({
      status: "ok",
      data: MID,
    });
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "  src/\n\ntests/\n  src/  \n" },
    });

    fireEvent.click(screen.getByRole("button", { name: /start/i }));
    await waitFor(() => {
      expect(commands.startMission).toHaveBeenCalledTimes(1);
    });
    const [spec] = vi.mocked(commands.startMission).mock.calls[0];
    expect(spec.scope_paths).toEqual(["src", "tests"]);
  });

  it("blocks submit and shows an inline error for an absolute scope path", async () => {
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "/etc/passwd" },
    });
    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    expect(commands.startMission).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/absolute/i);
    expect(screen.getByRole("alert").textContent).toMatch(/\/etc\/passwd/);
  });

  it("blocks submit and shows an inline error for parent-traversal", async () => {
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");

    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "../config\nsrc/../etc" },
    });
    fireEvent.click(screen.getByRole("button", { name: /start/i }));

    expect(commands.startMission).not.toHaveBeenCalled();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toMatch(/parent-traversal/i);
    // Both bad lines should be reported, not just the first — fixing
    // them one-by-one would be tedious.
    expect(alert.textContent).toMatch(/\.\.\/config/);
    expect(alert.textContent).toMatch(/src\/\.\.\/etc/);
  });

  it("clears scope-path errors as soon as the user edits the textarea", async () => {
    vi.mocked(openDialog).mockResolvedValueOnce("/tmp/project");
    render(<DeployPanel />);
    fireEvent.change(screen.getByLabelText(/mission objective/i), {
      target: { value: "Add /api/logout" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /browse for project folder/i }),
    );
    await waitFor(() => expect(screen.getByText("/tmp/project")).toBeTruthy());

    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "/bad" },
    });
    fireEvent.click(screen.getByRole("button", { name: /start/i }));
    expect(screen.getByRole("alert")).toBeTruthy();

    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "src/" },
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("scope-paths textarea content persists across mounts (localStorage prefs)", async () => {
    const first = render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/scope paths/i), {
      target: { value: "src/\ntests/" },
    });
    first.unmount();

    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    const ta = screen.getByLabelText(/scope paths/i) as HTMLTextAreaElement;
    expect(ta.value).toBe("src/\ntests/");
  });

  // P2-15: DevTools warns "A form field element should have an id or
  // name attribute" because aria-label alone doesn't satisfy autofill /
  // <label htmlFor> linkage. Every control must carry both id and name.
  it("every form control in the deploy panel has a non-empty id and name", () => {
    const { container } = render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "3" },
    });

    const panel = container.querySelector(".deploy-panel");
    expect(panel).not.toBeNull();
    const controls = panel!.querySelectorAll("input, select, textarea");
    expect(controls.length).toBeGreaterThan(0);
    for (const el of Array.from(controls)) {
      expect(el.id.length).toBeGreaterThan(0);
      expect(el.getAttribute("name")?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it("every form control id in the deploy panel is unique", () => {
    const { container } = render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "5" },
    });

    const panel = container.querySelector(".deploy-panel");
    const controls = panel!.querySelectorAll("input, select, textarea");
    const ids = Array.from(controls).map((el) => el.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("per-worker row ids include the index for both selects", () => {
    render(<DeployPanel />);
    fireEvent.click(screen.getByText(/^advanced$/i));
    fireEvent.change(screen.getByLabelText(/number of workers/i), {
      target: { value: "3" },
    });

    for (const i of [0, 1, 2]) {
      expect(document.getElementById(`deploy-worker-${i}-cli`)).not.toBeNull();
      expect(document.getElementById(`deploy-worker-${i}-model`)).not.toBeNull();
    }
  });

  it("objective textarea is linked via htmlFor and remains addressable by aria-label", () => {
    render(<DeployPanel />);
    const textarea = screen.getByLabelText(
      /mission objective/i,
    ) as HTMLTextAreaElement;
    expect(textarea.id).toBe("deploy-objective");
    const wrappingLabel = textarea.closest("label") as HTMLLabelElement | null;
    expect(wrappingLabel).not.toBeNull();
    expect(wrappingLabel?.getAttribute("for")).toBe("deploy-objective");
  });
});

describe("parseScopePaths", () => {
  it("returns empty arrays for empty input", () => {
    expect(parseScopePaths("")).toEqual({ paths: [], errors: [] });
    expect(parseScopePaths("   \n\n  \n")).toEqual({ paths: [], errors: [] });
  });

  it("trims whitespace and drops blank lines", () => {
    expect(parseScopePaths("  src/  \n\n tests/ \n")).toEqual({
      paths: ["src", "tests"],
      errors: [],
    });
  });

  it("preserves first-seen order and de-duplicates", () => {
    expect(parseScopePaths("src/\ntests/\nsrc/\ndocs/\ntests/")).toEqual({
      paths: ["src", "tests", "docs"],
      errors: [],
    });
  });

  it("rejects absolute paths", () => {
    const result = parseScopePaths("/etc/passwd\nsrc/");
    expect(result.paths).toEqual(["src"]);
    expect(result.errors).toEqual(["absolute path not allowed: /etc/passwd"]);
  });

  it("rejects any path containing a `..` segment", () => {
    const result = parseScopePaths("../foo\nsrc/../etc\nsrc/");
    expect(result.paths).toEqual(["src"]);
    expect(result.errors).toEqual([
      "parent-traversal not allowed: ../foo",
      "parent-traversal not allowed: src/../etc",
    ]);
  });

  it("does NOT reject `..` as a substring inside a segment", () => {
    // `..hidden` and `foo..bar` contain the literal `..` but are not
    // path-traversal — they are valid (if unusual) file names.
    expect(parseScopePaths("..hidden/\nfoo..bar")).toEqual({
      paths: ["..hidden", "foo..bar"],
      errors: [],
    });
  });

  it("normalizes current-directory and duplicate separators", () => {
    expect(parseScopePaths("./src\nsrc//core\nsrc/core")).toEqual({
      paths: ["src", "src/core"],
      errors: [],
    });
  });

  it("rejects root-equivalent and Windows-style paths", () => {
    const result = parseScopePaths(".\n./\nC:\\repo\nsrc\\core");
    expect(result.paths).toEqual([]);
    expect(result.errors).toHaveLength(4);
  });
});
