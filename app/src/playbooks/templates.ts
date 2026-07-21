import type { PlaybookTemplate } from "./types";

/// Three happy mock workers in one squad — the quickest demo of a
/// multi-agent room. The first member is visually designated as lead.
export const TRIO_SWEEP: PlaybookTemplate = {
  id: "builtin-trio-sweep",
  name: "Trio Sweep",
  description:
    "Three Claude-mock workers in one squad — quickest demo of a populated ops room.",
  squad: { name: "Trio Squad", color: "indigo" },
  members: [
    {
      role: "lead",
      spawn: { kind: "mock", script: "claude_happy" },
      isLead: true,
    },
    {
      role: "implementer",
      spawn: { kind: "mock", script: "claude_happy" },
    },
    {
      role: "reviewer",
      spawn: { kind: "mock", script: "claude_happy" },
    },
  ],
};

/// Three-vendor squad showing the full state palette in one click.
/// Replaces the original Aider-failing variant (removed in schema 2.0)
/// with `gemini_failed`, the new retryable-failure mock.
export const MIXED_DEMO: PlaybookTemplate = {
  id: "builtin-mixed-demo",
  name: "Mixed Demo",
  description:
    "Claude (happy) + Codex (blocked) + Gemini (failing) — full state palette in one squad.",
  squad: { name: "Demo Squad", color: "terracotta" },
  members: [
    {
      role: "lead",
      spawn: { kind: "mock", script: "claude_happy" },
      isLead: true,
    },
    {
      role: "blocker-on-deps",
      spawn: { kind: "mock", script: "codex_blocked" },
    },
    {
      role: "fault-injector",
      spawn: { kind: "mock", script: "gemini_failed" },
    },
  ],
};

/// A two-vendor squad showing the compact two-worker layout.
export const HANDOFF_PAIR: PlaybookTemplate = {
  id: "builtin-handoff-pair",
  name: "Pair Demo",
  description: "Claude- and Codex-style mock workers in one compact squad.",
  squad: { name: "Pair Squad", color: "sage" },
  members: [
    {
      role: "lead",
      spawn: { kind: "mock", script: "claude_happy" },
      isLead: true,
    },
    {
      role: "implementer",
      spawn: { kind: "mock", script: "codex_blocked" },
    },
  ],
};

export const BUILTIN_PLAYBOOKS: readonly PlaybookTemplate[] = [
  TRIO_SWEEP,
  MIXED_DEMO,
  HANDOFF_PAIR,
];
