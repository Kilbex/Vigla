import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Event } from "../../bindings";
import FilesTab from "../FilesTab";

vi.mock("../../bindings", () => ({
  commands: {
    getWorkerDiff: vi.fn().mockResolvedValue({ status: "ok", data: "diff" }),
  },
}));

import { commands } from "../../bindings";

function fileEvent(seq: number, op: "created" | "modified" | "deleted"): Event {
  return {
    schema_version: "1.0",
    worker_id: "worker-1",
    task_id: null,
    seq,
    ts: "2026-07-21T12:00:00Z",
    type: "file_activity",
    payload: { op, path: `file-${seq}.txt`, lines_added: 1, lines_removed: 0 },
  };
}

function completion(seq: number): Event {
  return {
    schema_version: "1.0",
    worker_id: "worker-1",
    task_id: null,
    seq,
    ts: "2026-07-21T12:00:01Z",
    type: "completion",
    payload: { summary: "done" },
  };
}

beforeEach(() => vi.clearAllMocks());

describe("FilesTab", () => {
  it("counts file operations and refreshes the diff on terminal evidence", async () => {
    const initial = [
      fileEvent(1, "created"),
      fileEvent(2, "modified"),
      fileEvent(3, "deleted"),
    ];
    const view = render(<FilesTab events={initial} workerId="worker-1" />);
    expect(screen.getByText("Added").nextSibling?.textContent).toBe("1");
    expect(screen.getByText("Modified").nextSibling?.textContent).toBe("1");
    expect(screen.getByText("Deleted").nextSibling?.textContent).toBe("1");
    await waitFor(() => expect(commands.getWorkerDiff).toHaveBeenCalledTimes(1));

    view.rerender(
      <FilesTab events={[...initial, completion(4)]} workerId="worker-1" />,
    );
    await waitFor(() => expect(commands.getWorkerDiff).toHaveBeenCalledTimes(2));
  });
});
