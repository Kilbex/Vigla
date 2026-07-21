import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import HudMark from "../HudMark";

describe("HudMark — shared decorative reticle (P2-19)", () => {
  it("renders an SVG with the given size", () => {
    const { container } = render(<HudMark size={48} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg!.getAttribute("width")).toBe("48");
    expect(svg!.getAttribute("height")).toBe("48");
    expect(svg!.getAttribute("viewBox")).toBe("0 0 48 48");
  });

  it("renders at a larger size while preserving the 48-unit viewBox", () => {
    const { container } = render(<HudMark size={160} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg!.getAttribute("width")).toBe("160");
    expect(svg!.getAttribute("height")).toBe("160");
    // viewBox is fixed at the inner coordinate space so the shape
    // scales cleanly with width/height.
    expect(svg!.getAttribute("viewBox")).toBe("0 0 48 48");
  });

  it("applies the given className to the outer <svg>", () => {
    const { container } = render(
      <HudMark size={48} className="custom-class operations-room__compass" />,
    );
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg!.getAttribute("class")).toContain("custom-class");
    expect(svg!.getAttribute("class")).toContain("operations-room__compass");
  });

  it("marks the outer SVG as aria-hidden (decorative only)", () => {
    const { container } = render(<HudMark size={48} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg!.getAttribute("aria-hidden")).toBe("true");
  });
});
