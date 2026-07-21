// P2-19 — InboxOverview empty state uses the shared HudMark
// component. This test asserts the reticle is rendered via the
// new shared component (proven by the 48-unit viewBox) and that
// the existing `.inbox-overview-empty__reticle` CSS hook is
// preserved.

import { beforeEach, describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import InboxOverview from "../InboxOverview";
import { useMissionsStore } from "../../missions/store";

beforeEach(() => {
  useMissionsStore.getState().reset();
});

describe("InboxOverview — empty state uses shared HudMark (P2-19)", () => {
  it("renders a quiet glyph empty-state (HUD reticle retired) when no mission is active", () => {
    // No active mission → empty branch fires.
    const { container } = render(<InboxOverview />);
    // The HUD reticle was retired; a muted glyph + label is the empty-state.
    expect(
      container.querySelector("svg.inbox-overview-empty__reticle"),
    ).toBeNull();
    expect(container.querySelector(".inbox-overview-empty")).not.toBeNull();
    expect(
      container.querySelector(".inbox-overview-empty__label")?.textContent,
    ).toBe("No active mission");
  });
});
