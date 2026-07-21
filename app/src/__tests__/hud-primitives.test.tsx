import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";

describe("HUD primitives", () => {
  // Smoke check: hud.css must be loaded into the test environment
  // so any surface that opts into `hud-corners` etc. actually has the
  // class definitions available. We assert by class string only — the
  // actual visual is verified manually via `pnpm dev`.
  it("renders an element with the hud-corners class without throwing", () => {
    const { container } = render(
      <div className="hud-corners" data-testid="probe">
        <span className="hud-chroma">PROBE</span>
      </div>,
    );
    const probe = container.querySelector('[data-testid="probe"]');
    expect(probe).not.toBeNull();
    expect(probe).toHaveClass("hud-corners");
  });
});
