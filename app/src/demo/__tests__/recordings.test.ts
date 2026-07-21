import { describe, expect, it } from "vitest";
import { DEMO_RECORDINGS, findDemoRecording } from "../recordings";

describe("web-demo recordings", () => {
  it("ships the accepted, blocked, and quota-paused outcomes", () => {
    expect(DEMO_RECORDINGS.map((recording) => recording.id)).toEqual([
      "happy",
      "blocked",
      "quota",
    ]);
  });

  it("keeps every stream deterministic and canonical", () => {
    for (const recording of DEMO_RECORDINGS) {
      expect(recording.events.length).toBeGreaterThan(0);
      expect(recording.events.every((event) => event.schema_version === "2.0")).toBe(true);
      expect(recording.events.some((event) => /gemini/i.test(event.worker_id))).toBe(false);

      const timestamps = recording.events.map((event) => Date.parse(event.ts));
      expect(timestamps).toEqual([...timestamps].sort((a, b) => a - b));

      const lastSeq = new Map<string, number>();
      for (const event of recording.events) {
        expect(event.seq).toBe((lastSeq.get(event.worker_id) ?? 0) + 1);
        lastSeq.set(event.worker_id, event.seq);
      }
    }
  });

  it("rejects an unknown recording id", () => {
    expect(() => findDemoRecording("missing" as never)).toThrow(
      "unknown demo recording",
    );
  });
});
