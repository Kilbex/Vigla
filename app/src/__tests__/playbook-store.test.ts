import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../bindings", () => ({
  commands: {
    listPlaybooks: vi.fn(),
    savePlaybook: vi.fn(),
    deletePlaybook: vi.fn(),
  },
}));

import { commands } from "../bindings";
import {
  deletePlaybook,
  loadAllPlaybooks,
  parsePlaybookJson,
  savePlaybook,
} from "../playbooks/store";
import { BUILTIN_PLAYBOOKS } from "../playbooks/templates";

beforeEach(() => {
  vi.clearAllMocks();
});

describe("parsePlaybookJson — happy paths", () => {
  it("accepts a minimal valid mock playbook", () => {
    const json = JSON.stringify({
      id: "x",
      name: "X",
      description: "min",
      squad: { name: "X Squad", color: "indigo" },
      members: [
        { role: "lead", spawn: { kind: "mock", script: "claude_happy" } },
      ],
    });
    const r = parsePlaybookJson(json);
    expect("ok" in r).toBe(true);
    if ("ok" in r) {
      expect(r.ok.id).toBe("x");
      expect(r.ok.members.length).toBe(1);
    }
  });

  it("accepts a multi-vendor playbook with claude + codex members", () => {
    const json = JSON.stringify({
      id: "multi",
      name: "Multi",
      description: "mixed",
      squad: { name: "Mixed", color: "sage" },
      members: [
        {
          role: "lead",
          spawn: { kind: "claude", prompt: "do it", cwd: "/tmp", maxTurns: 4 },
          isLead: true,
        },
        {
          role: "doer",
          spawn: { kind: "codex", prompt: "follow up", cwd: "/tmp" },
        },
      ],
    });
    const r = parsePlaybookJson(json);
    expect("ok" in r).toBe(true);
  });

  it("round-trips every BUILTIN_PLAYBOOK through serialize → parse", () => {
    for (const t of BUILTIN_PLAYBOOKS) {
      const json = JSON.stringify(t);
      const r = parsePlaybookJson(json);
      expect("ok" in r).toBe(true);
      if ("ok" in r) {
        expect(r.ok.name).toBe(t.name);
        expect(r.ok.members.length).toBe(t.members.length);
      }
    }
  });
});

describe("parsePlaybookJson — schema errors", () => {
  it("rejects non-JSON input", () => {
    const r = parsePlaybookJson("{not valid");
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/not valid JSON/);
  });

  it("rejects an empty playbook id", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "",
        name: "x",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [
          { role: "r", spawn: { kind: "mock", script: "claude_happy" } },
        ],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/playbook\.id/);
  });

  it("rejects an unknown squad color", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "x",
        name: "X",
        description: "",
        squad: { name: "s", color: "neon-pink" },
        members: [
          { role: "r", spawn: { kind: "mock", script: "claude_happy" } },
        ],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/squad\.color invalid/);
  });

  it("rejects an empty members array", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "x",
        name: "X",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/members cannot be empty/);
  });

  it("rejects a member with an unknown spawn kind", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "x",
        name: "X",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [{ role: "r", spawn: { kind: "totally-fake" } }],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/spawn\.kind/);
  });

  it("rejects a claude member missing prompt", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "x",
        name: "X",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [{ role: "r", spawn: { kind: "claude", cwd: "/tmp" } }],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/prompt/);
  });

  it("rejects a member with non-boolean isLead", () => {
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "x",
        name: "X",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [
          {
            role: "r",
            spawn: { kind: "mock", script: "claude_happy" },
            isLead: "yes",
          },
        ],
      }),
    );
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toMatch(/isLead/);
  });

  it("accepts unknown future MockScript values (forward-compat)", () => {
    // Audit-r5 vendorOf fallback returns "mock" for unknown scripts;
    // the parser similarly should not lock to the in-tree union so
    // disk-backed playbooks from a future build keep loading.
    const r = parsePlaybookJson(
      JSON.stringify({
        id: "future",
        name: "Future",
        description: "",
        squad: { name: "s", color: "indigo" },
        members: [
          { role: "r", spawn: { kind: "mock", script: "future_script_v2" } },
        ],
      }),
    );
    expect("ok" in r).toBe(true);
  });
});

describe("loadAllPlaybooks — IPC integration", () => {
  it("returns built-ins only when listPlaybooks returns empty", async () => {
    (commands.listPlaybooks as any).mockResolvedValue({
      status: "ok",
      data: [],
    });
    const list = await loadAllPlaybooks();
    expect(list.length).toBe(BUILTIN_PLAYBOOKS.length);
    expect(list.every((e) => e.source === "builtin")).toBe(true);
  });

  it("returns built-ins only when listPlaybooks errors (degrade gracefully)", async () => {
    (commands.listPlaybooks as any).mockResolvedValue({
      status: "error",
      error: "disk unreadable",
    });
    const list = await loadAllPlaybooks();
    expect(list.length).toBe(BUILTIN_PLAYBOOKS.length);
    expect(list.every((e) => e.source === "builtin")).toBe(true);
  });

  it("appends valid saved playbooks after built-ins", async () => {
    (commands.listPlaybooks as any).mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "saved-1",
          json: JSON.stringify({
            id: "saved-1",
            name: "Saved 1",
            description: "",
            squad: { name: "s", color: "indigo" },
            members: [
              { role: "r", spawn: { kind: "mock", script: "claude_happy" } },
            ],
          }),
        },
      ],
    });
    const list = await loadAllPlaybooks();
    expect(list.length).toBe(BUILTIN_PLAYBOOKS.length + 1);
    expect(list[list.length - 1].source).toBe("saved");
    expect(list[list.length - 1].template.id).toBe("saved-1");
  });

  it("skips malformed saved playbooks, keeps the rest", async () => {
    (commands.listPlaybooks as any).mockResolvedValue({
      status: "ok",
      data: [
        { id: "bad", json: "{not valid" },
        {
          id: "good",
          json: JSON.stringify({
            id: "good",
            name: "Good",
            description: "",
            squad: { name: "s", color: "indigo" },
            members: [
              { role: "r", spawn: { kind: "mock", script: "claude_happy" } },
            ],
          }),
        },
      ],
    });
    // Suppress the warn spam in tests.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const list = await loadAllPlaybooks();
    warn.mockRestore();
    expect(list.length).toBe(BUILTIN_PLAYBOOKS.length + 1);
    expect(list[list.length - 1].template.id).toBe("good");
  });

  it("forces saved id to match the on-disk filename (overrides body.id)", async () => {
    (commands.listPlaybooks as any).mockResolvedValue({
      status: "ok",
      data: [
        {
          id: "filename-id",
          json: JSON.stringify({
            id: "DIFFERENT-id-in-body",
            name: "X",
            description: "",
            squad: { name: "s", color: "indigo" },
            members: [
              { role: "r", spawn: { kind: "mock", script: "claude_happy" } },
            ],
          }),
        },
      ],
    });
    const list = await loadAllPlaybooks();
    const saved = list.find((e) => e.source === "saved");
    expect(saved?.template.id).toBe("filename-id");
  });
});

describe("savePlaybook / deletePlaybook — IPC wrappers", () => {
  it("savePlaybook serializes the template with indentation and forwards to IPC", async () => {
    (commands.savePlaybook as any).mockResolvedValue({
      status: "ok",
      data: null,
    });
    const r = await savePlaybook({
      id: "x",
      name: "X",
      description: "",
      squad: { name: "s", color: "indigo" },
      members: [{ role: "r", spawn: { kind: "mock", script: "claude_happy" } }],
    });
    expect("ok" in r).toBe(true);
    expect(commands.savePlaybook).toHaveBeenCalledWith("x", expect.any(String));
    const json = (commands.savePlaybook as any).mock.calls[0][1];
    expect(json).toContain("\n"); // pretty-printed
    expect(JSON.parse(json).name).toBe("X");
  });

  it("savePlaybook surfaces IPC error string as { err }", async () => {
    (commands.savePlaybook as any).mockResolvedValue({
      status: "error",
      error: "disk full",
    });
    const r = await savePlaybook({
      id: "x",
      name: "X",
      description: "",
      squad: { name: "s", color: "indigo" },
      members: [{ role: "r", spawn: { kind: "mock", script: "claude_happy" } }],
    });
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toBe("disk full");
  });

  it("deletePlaybook forwards to IPC and surfaces errors", async () => {
    (commands.deletePlaybook as any).mockResolvedValue({
      status: "error",
      error: "permission denied",
    });
    const r = await deletePlaybook("x");
    expect("err" in r).toBe(true);
    if ("err" in r) expect(r.err).toBe("permission denied");
  });
});
