// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { downloadMindMapSvg } from "../plan-mind-map-export";

const mocks = vi.hoisted(() => ({
  saveMindMapFile: vi.fn(),
  rendererSaveDialog: vi.fn(),
}));

vi.mock("../../bindings", () => ({
  commands: {
    saveMindMapFile: mocks.saveMindMapFile,
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: mocks.rendererSaveDialog,
}));

describe("mind-map export authority", () => {
  beforeEach(() => {
    mocks.saveMindMapFile.mockReset();
    mocks.saveMindMapFile.mockResolvedValue({ status: "ok", data: null });
    mocks.rendererSaveDialog.mockReset();
    mocks.rendererSaveDialog.mockResolvedValue("/tmp/renderer-selected.svg");
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => "blob:fallback"),
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: vi.fn(),
    });
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
  });

  afterEach(() => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    vi.restoreAllMocks();
  });

  it("lets the host choose the destination instead of accepting a renderer path", async () => {
    await downloadMindMapSvg({
      title: "Secure export",
      nodes: [],
      edges: [],
      bounds: { width: 1, height: 1 },
    });

    expect(mocks.rendererSaveDialog).not.toHaveBeenCalled();
    expect(mocks.saveMindMapFile).toHaveBeenCalledWith(
      "secure-export-mind-map.svg",
      expect.stringContaining("<svg"),
    );
  });
});
