import { describe, it, expect, beforeEach, vi } from "vitest";
import { render } from "@testing-library/react";
import { ReactFlowProvider } from "@xyflow/react";
import OperationsRoom from "../operations/OperationsRoom";
import { useOpsStore } from "../store";
import { useMissionsStore } from "../missions/store";
import { __resetWorkerIdentityCache } from "../operations/useWorkerIdentity";

// P1-13 — the bindings layer needs to exist for the identity hook
// rendered inside each Station tile; default to "missing" so it
// doesn't enrich.
//
// P2-19 — the launch-empty branch mounts DeployPanel, which probes
// CLI auth on mount; stub that too so the empty-state render is
// deterministic.
vi.mock("../bindings", () => {
  return {
    commands: {
      getWorkerInfo: vi.fn().mockResolvedValue({ status: "ok", data: null }),
      checkCliAuth: vi.fn().mockResolvedValue([]),
      openCliLogin: vi.fn(),
      startMission: vi.fn(),
    },
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

beforeEach(() => {
  useOpsStore.getState().reset();
  useMissionsStore.getState().reset();
  __resetWorkerIdentityCache();
});

describe("OperationsRoom — HUD watermark removed (P1-13)", () => {
  function seedWorker() {
    useOpsStore.getState().registerWorker("w-room", "claude", "do thing");
  }

  it("does not render the operations-room__hud-overlay block", () => {
    seedWorker();
    const { container } = render(
      <ReactFlowProvider>
        <OperationsRoom />
      </ReactFlowProvider>,
    );
    expect(
      container.querySelector(".operations-room__hud-overlay"),
    ).toBeNull();
  });

  it("does not include the SF coordinates or SYS.FEED watermark text", () => {
    seedWorker();
    const { container } = render(
      <ReactFlowProvider>
        <OperationsRoom />
      </ReactFlowProvider>,
    );
    const text = container.textContent ?? "";
    expect(text).not.toMatch(/MATRIX SECTOR/);
    expect(text).not.toMatch(/37°46'N/);
    expect(text).not.toMatch(/SYS\.FEED/);
  });
});

describe("OperationsRoom — launch-empty uses shared HudMark (P2-19)", () => {
  it("renders the compass class via the shared 48-viewBox HudMark", () => {
    // No workers (store was reset), no active mission → launch-empty
    // branch fires and renders the compass.
    const { container } = render(
      <ReactFlowProvider>
        <OperationsRoom />
      </ReactFlowProvider>,
    );
    const compass = container.querySelector("svg.operations-room__compass");
    expect(compass).not.toBeNull();
    // The shared HudMark uses a fixed 48-unit coordinate space and
    // scales via width/height. Proves we're rendering the new
    // shared component, not the old 160×160 compass markup.
    expect(compass!.getAttribute("viewBox")).toBe("0 0 48 48");
    expect(compass!.getAttribute("width")).toBe("160");
    expect(compass!.getAttribute("height")).toBe("160");
  });
});
