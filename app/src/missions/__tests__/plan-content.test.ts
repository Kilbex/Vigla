import { describe, expect, it } from "vitest";
import {
  PLAN_CONTENT_LIMITS,
  sanitizePlanDetail,
  sanitizePlanLabel,
} from "../plan-content";

describe("plan content sanitization", () => {
  it("removes display markup, controls, and bidirectional overrides", () => {
    expect(
      sanitizePlanLabel(
        '`**Review** <b>auth</b><br>flow\u0000\u001b\u202Espoofed`',
      ),
    ).toBe("Review auth flowspoofed");
  });

  it("decodes entities once without turning encoded markup into a tag", () => {
    expect(sanitizePlanLabel("&amp;lt;b&amp;gt; literal")).toBe(
      "&lt;b&gt; literal",
    );
    expect(sanitizePlanLabel("Review &lt;T&gt; &amp; ship")).toBe(
      "Review <T> & ship",
    );
  });

  it("keeps technical angle brackets while stripping paired display markup", () => {
    expect(
      sanitizePlanLabel("Return Vec<String> from <script>alert(1)</script>"),
    ).toBe("Return Vec<String> from alert(1)");
    expect(sanitizePlanLabel("<div><p>Review plan</p></div>")).toBe(
      "Review plan",
    );
    expect(sanitizePlanLabel("Fix <button> inside <form>")).toBe(
      "Fix <button> inside <form>",
    );
  });

  it("preserves legitimate joiners used by emoji and global scripts", () => {
    expect(sanitizePlanLabel("Ship 👩‍💻 build")).toBe("Ship 👩‍💻 build");
  });

  it("caps untrusted detail by Unicode code point without splitting emoji", () => {
    const source = `${"a".repeat(PLAN_CONTENT_LIMITS.detailCodePoints)}🙂tail`;
    const result = sanitizePlanDetail(source);
    expect(Array.from(result)).toHaveLength(
      PLAN_CONTENT_LIMITS.detailCodePoints,
    );
    expect(result.endsWith("…")).toBe(true);
    expect(result).not.toContain("�");
  });

  it("bounds sanitizer work before processing oversized model output", () => {
    const oversized = `${"<b></b> ".repeat(20_000)}late content`;
    expect(sanitizePlanLabel(oversized)).toBe("");
  });
});
