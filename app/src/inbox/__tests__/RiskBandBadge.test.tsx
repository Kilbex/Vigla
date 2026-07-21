// S10 — RiskBandBadge component test. Pure render; asserts the
// label + class combination for each band so the colour mapping
// stays explicit.

import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import RiskBandBadge from "../RiskBandBadge";

describe("RiskBandBadge", () => {
  it.each([
    ["low", /low risk/i],
    ["medium", /medium risk/i],
    ["high", /high risk/i],
  ] as const)(
    "renders %s band with correct label and class",
    (band, label) => {
      const { container, getByText } = render(<RiskBandBadge band={band} />);
      expect(getByText(label)).toBeTruthy();
      expect(container.querySelector(`.risk-band-badge--${band}`)).not.toBeNull();
      expect(container.querySelector(".risk-band-dot")).not.toBeNull();
    },
  );

  it("falls back to medium when given an unknown band (defensive)", () => {
    // @ts-expect-error — exercising the runtime defensive path.
    const { container } = render(<RiskBandBadge band="unknown" />);
    expect(container.querySelector(".risk-band-badge--medium")).not.toBeNull();
  });
});
