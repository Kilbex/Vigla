import { describe, it, expect, beforeEach } from "vitest";
import { getPlanMode, setPlanMode, type PlanMode } from "../preferences";

describe("plan_mode preference", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("defaults to 'direct'", () => {
    expect(getPlanMode()).toBe<PlanMode>("direct");
  });

  it("round-trips through setPlanMode", () => {
    setPlanMode("review");
    expect(getPlanMode()).toBe<PlanMode>("review");

    setPlanMode("direct");
    expect(getPlanMode()).toBe<PlanMode>("direct");
  });

  it("falls back to 'direct' for unknown legacy storage values", () => {
    window.localStorage.setItem("vigla.prefs.plan_mode.v1", "junk");
    expect(getPlanMode()).toBe<PlanMode>("direct");
  });

  it("notifies subscribers on setPlanMode", async () => {
    const { usePlanMode } = await import("../preferences");
    const { renderHook, act } = await import("@testing-library/react");
    const { result } = renderHook(() => usePlanMode());

    expect(result.current[0]).toBe("direct");

    act(() => {
      result.current[1]("review");
    });

    expect(result.current[0]).toBe("review");
  });
});
