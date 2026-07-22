import { describe, expect, it } from "vitest";
import {
  CLI_MODEL_OPTIONS,
  encodeWorkerModelRoster,
  normalizeWorkerCliModel,
} from "../comms/deploy-models";

describe("worker CLI model catalog", () => {
  it("offers the current Claude Code aliases with the requested guidance", () => {
    expect(CLI_MODEL_OPTIONS.claude).toEqual([
      {
        value: null,
        label: "Default (recommended)",
        detail:
          "Opus 4.8 with 1M context · Best for everyday, complex tasks",
      },
      {
        value: "opus",
        label: "Opus",
        detail:
          "Opus 4.8 with 1M context · Best for everyday, complex tasks",
      },
      {
        value: "fable",
        label: "Fable",
        detail:
          "Fable 5 · Most capable for your hardest and longest-running tasks",
      },
      {
        value: "sonnet",
        label: "Sonnet",
        detail: "Sonnet 5 · Efficient for routine tasks",
      },
      {
        value: "haiku",
        label: "Haiku",
        detail: "Haiku 4.5 · Fastest for quick answers",
      },
    ]);
  });

  it("offers every current GPT-5.6 Codex tier before older compatible models", () => {
    expect(CLI_MODEL_OPTIONS.codex.slice(0, 4)).toEqual([
      {
        value: null,
        label: "Default",
        detail: "Use Codex CLI's current account and configuration default",
      },
      {
        value: "gpt-5.6-sol",
        label: "GPT-5.6 Sol",
        detail: "Flagship capability for complex and long-running work",
      },
      {
        value: "gpt-5.6-terra",
        label: "GPT-5.6 Terra",
        detail: "Balanced capability, speed, and cost for everyday work",
      },
      {
        value: "gpt-5.6-luna",
        label: "GPT-5.6 Luna",
        detail: "Fastest, most cost-efficient GPT-5.6 option",
      },
    ]);
    expect(CLI_MODEL_OPTIONS.codex.map(({ value }) => value)).toContain(
      "gpt-5.5",
    );
  });

  it("preserves and encodes current selections for each CLI", () => {
    expect(normalizeWorkerCliModel("claude", "fable")).toBe("fable");
    expect(normalizeWorkerCliModel("codex", "gpt-5.6-sol")).toBe(
      "gpt-5.6-sol",
    );
    expect(
      encodeWorkerModelRoster(
        "2",
        ["claude", "codex"],
        ["fable", "gpt-5.6-sol"],
      ),
    ).toBe("claude:fable,codex:gpt-5.6-sol");
  });

  it("keeps previously supported saved selections pinned", () => {
    expect(normalizeWorkerCliModel("claude", "sonnet")).toBe("sonnet");
    expect(normalizeWorkerCliModel("codex", "gpt-5.4-mini")).toBe(
      "gpt-5.4-mini",
    );
    expect(
      encodeWorkerModelRoster(
        "2",
        ["claude", "codex"],
        ["sonnet", "gpt-5.4-mini"],
      ),
    ).toBe("claude:sonnet,codex:gpt-5.4-mini");
  });
});
