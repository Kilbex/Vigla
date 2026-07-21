import { describe, it, expect } from "vitest";
import {
  formatAvatarLabel,
  getAnimationProfile,
  getAvatarProfile,
  getAvatarType,
  getRoleBadge,
  getStateOverlay,
  getVendorGlyph,
} from "../operations/avatar";
import type { Vendor, WorkerState } from "../bindings";

const VENDORS: Array<{ vendor: Vendor; letter: string; role: string; avatarType: "portrait" | "placeholder" }> = [
  { vendor: "claude",   letter: "C", role: "strategist", avatarType: "portrait" },
  { vendor: "codex",    letter: "X", role: "engineer",   avatarType: "portrait" },
  { vendor: "gemini",   letter: "G", role: "analyst",    avatarType: "portrait" },
  { vendor: "opencode", letter: "O", role: "executor",   avatarType: "placeholder" },
  { vendor: "mock",     letter: "M", role: "stand-in",   avatarType: "placeholder" },
];

const STATES: Array<{ state: WorkerState; animation: string }> = [
  { state: "idle", animation: "breath" },
  { state: "planning", animation: "pulse" },
  { state: "executing", animation: "spin" },
  { state: "blocked", animation: "blink" },
  { state: "reviewing", animation: "pulse" },
  { state: "done", animation: "lock-in" },
  { state: "failed", animation: "static" },
];

describe("avatar — vendor mapping", () => {
  for (const { vendor, letter, role, avatarType } of VENDORS) {
    it(`maps vendor '${vendor}' to a distinct glyph + role + avatar type`, () => {
      const glyph = getVendorGlyph(vendor);
      expect(glyph.letter).toBe(letter);
      expect(glyph.hue).toBe(vendor);
      expect(glyph.vendorLabel.length).toBeGreaterThan(0);
      expect(getRoleBadge(vendor)).toBe(role);
      expect(getAvatarType(vendor)).toBe(avatarType);
    });
  }

  it("vendor letters are pairwise distinct (no collision across vendors)", () => {
    const letters = VENDORS.map((v) => getVendorGlyph(v.vendor).letter);
    expect(new Set(letters).size).toBe(letters.length);
  });

  it("vendor hues are pairwise distinct (no collision across vendors)", () => {
    const hues = VENDORS.map((v) => getVendorGlyph(v.vendor).hue);
    expect(new Set(hues).size).toBe(hues.length);
  });

  it("falls back safely on unknown vendor strings", () => {
    // Future vendor not yet known to the schema. Forward-compat path.
    const glyph = getVendorGlyph("future-vendor" as Vendor);
    expect(glyph.letter).toBe("?");
    expect(glyph.hue).toBe("unknown");
    expect(getRoleBadge("future-vendor" as Vendor)).toBe("unknown");
    // Unknown vendors fall back to the letter-glyph placeholder — only
    // the three real-vendor CLIs have hand-pixelled portrait sprites.
    expect(getAvatarType("future-vendor" as Vendor)).toBe("placeholder");
  });
});

describe("avatar — state mapping", () => {
  for (const { state, animation } of STATES) {
    it(`maps state '${state}' to overlay + ${animation} animation`, () => {
      expect(getStateOverlay(state)).toBe(state);
      expect(getAnimationProfile(state)).toBe(animation);
    });
  }

  it("falls back safely on unknown worker state strings", () => {
    expect(getStateOverlay("future-state" as WorkerState)).toBe("unknown");
    expect(getAnimationProfile("future-state" as WorkerState)).toBe("static");
  });
});

describe("avatar — combined profile", () => {
  it("composes a portrait profile for the three real-vendor CLIs", () => {
    const profile = getAvatarProfile("claude", "executing");
    expect(profile.avatarType).toBe("portrait");
    expect(profile.vendorGlyph.letter).toBe("C");
    expect(profile.roleBadge).toBe("strategist");
    expect(profile.stateOverlay).toBe("executing");
    expect(profile.animationProfile).toBe("spin");
    expect(profile.label).toMatch(/Claude worker/);
    expect(profile.label).toMatch(/strategist/);
    expect(profile.label).toMatch(/executing/);
  });

  it("composes a placeholder profile for vendors without portraits", () => {
    const profile = getAvatarProfile("mock", "executing");
    expect(profile.avatarType).toBe("placeholder");
    expect(profile.vendorGlyph.letter).toBe("M");
  });

  it("renders a 'unknown state' label token for unknown states", () => {
    const profile = getAvatarProfile("mock", "future-state" as WorkerState);
    expect(profile.stateOverlay).toBe("unknown");
    expect(profile.label).toMatch(/unknown state/);
  });

  it("is referentially deterministic for the same inputs", () => {
    const a = getAvatarProfile("gemini", "blocked");
    const b = getAvatarProfile("gemini", "blocked");
    expect(a).toEqual(b);
  });

  it("formatAvatarLabel composes label parts", () => {
    const label = formatAvatarLabel(
      { letter: "G", hue: "gemini", vendorLabel: "Gemini" },
      "analyst",
      "reviewing",
    );
    expect(label).toBe("Gemini worker · analyst · reviewing");
  });
});
