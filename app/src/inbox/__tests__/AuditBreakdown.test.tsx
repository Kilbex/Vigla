// S10 — AuditBreakdown component test. Parses an AuditReport
// payload_json and asserts that each scorer renders a row with
// its score; absent (None) scorers render as "skipped".

import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import AuditBreakdown from "../AuditBreakdown";

const PAYLOAD_FULL = JSON.stringify({
  overall: 0.82,
  test_pass: { ran: true, passed: 12, failed: 0, skipped: 1, score: 1.0 },
  scope: { in_scope: 5, out_of_scope: 0, score: 1.0 },
  regression: {
    baseline_passed: true,
    current_passed: true,
    newly_failing: [],
    newly_passing: [],
    score: 1.0,
  },
  lint: { rustfmt_clean: true, clippy_warnings: 0, biome_diagnostics: null, score: 0.95 },
  security_flags: [],
});

const PAYLOAD_PARTIAL = JSON.stringify({
  overall: 0.6,
  test_pass: { ran: true, passed: 3, failed: 2, skipped: 0, score: 0.6 },
  scope: null,
  regression: null,
  lint: null,
  security_flags: [],
});

describe("AuditBreakdown", () => {
  it("renders the composite overall score + tier", () => {
    const { getByText } = render(
      <AuditBreakdown tier="standard" payloadJson={PAYLOAD_FULL} />,
    );
    expect(getByText("0.82")).toBeTruthy();
    expect(getByText(/STANDARD/)).toBeTruthy();
  });

  it("renders all four scorer rows when full payload supplied", () => {
    const { getByText } = render(
      <AuditBreakdown tier="deep" payloadJson={PAYLOAD_FULL} />,
    );
    for (const name of ["Tests", "Scope", "Regression", "Lint"]) {
      expect(getByText(new RegExp(`^${name}$`))).toBeTruthy();
    }
  });

  it("exposes numeric and skipped scores as accessible meters", () => {
    const { getByRole } = render(
      <AuditBreakdown tier="smoke" payloadJson={PAYLOAD_PARTIAL} />,
    );

    expect(getByRole("meter", { name: "Tests audit score" })).toHaveAttribute(
      "aria-valuenow",
      "0.6",
    );
    expect(getByRole("meter", { name: "Scope audit score" })).toHaveAttribute(
      "aria-valuetext",
      "skipped",
    );
  });

  it("marks absent scorers as skipped", () => {
    const { container } = render(
      <AuditBreakdown tier="smoke" payloadJson={PAYLOAD_PARTIAL} />,
    );
    // scope + regression + lint absent in PAYLOAD_PARTIAL → 3 skipped
    expect(container.querySelectorAll(".audit-breakdown-row--skipped").length).toBe(3);
  });

  it("renders empty placeholder for null payload, error placeholder for malformed", () => {
    const a = render(<AuditBreakdown tier="smoke" payloadJson={null} />);
    expect(a.getByText(/awaiting audit/i)).toBeTruthy();
    const b = render(<AuditBreakdown tier="smoke" payloadJson="not json" />);
    expect(b.getByText(/audit unavailable/i)).toBeTruthy();
  });

  it("applies threshold-coloured classes to the overall score", () => {
    const good = render(<AuditBreakdown tier="standard" payloadJson={PAYLOAD_FULL} />);
    expect(good.container.querySelector(".audit-breakdown-overall--good")).not.toBeNull();
    const warn = render(<AuditBreakdown tier="standard" payloadJson={PAYLOAD_PARTIAL} />);
    expect(warn.container.querySelector(".audit-breakdown-overall--warning")).not.toBeNull();
  });

  it("degrades off-contract score payloads instead of throwing", () => {
    const payload = JSON.stringify({
      overall: "bad",
      test_pass: { ran: true, passed: 2, failed: 1, skipped: 0 },
      scope: { in_scope: 1, out_of_scope: 0, score: "0.8" },
      regression: { newly_failing: "not-an-array", score: 0.4 },
      lint: { score: null },
      security_flags: [],
    });

    const { container, getByText } = render(
      <AuditBreakdown tier="standard" payloadJson={payload} />,
    );

    expect(getByText("0.00")).toBeTruthy();
    expect(container.querySelectorAll(".audit-breakdown-row--skipped").length).toBe(3);
    expect(container.querySelector('[title="0 newly failing"]')).not.toBeNull();
  });
});
