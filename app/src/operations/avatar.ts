import type { Vendor, WorkerState } from "../bindings";

/// Replaceable placeholder avatar rules. Pure projection of
/// `(Vendor, WorkerState)` into an `AvatarProfile`. Future cartoon
/// avatars consume the same profile shape — only `avatarType` flips
/// from `"placeholder"` to a richer kind, and `WorkerAvatar` learns to
/// render the new kind. Schema, persistence, and the event boundary
/// are untouched.

/// Two avatar kinds live side-by-side:
/// - `"placeholder"` — letter-glyph badge (still used by `opencode`, `mock`,
///   and the forward-compat `unknown` fallback).
/// - `"portrait"` — hand-pixelled operator portrait (used by the three
///   real-vendor CLIs: claude, codex, gemini). Sprites live under
///   `app/src/assets/avatars/`; source-of-truth + design rationale in
///   `docs/character-designs/`.
export type AvatarType = "placeholder" | "portrait";

/// Vendors that have a hand-pixelled portrait sprite. Stays in sync with
/// the SVGs under `app/src/assets/avatars/`.
const PORTRAIT_VENDORS: ReadonlySet<VendorHue> = new Set(["claude", "codex", "gemini"]);

/// Closed set of vendor color hue tokens. Matches the CSS variables
/// `--vendor-{hue}` in `index.css`. `unknown` is the safe fallback.
export type VendorHue =
  | "claude"
  | "codex"
  | "gemini"
  | "antigravity"
  | "kiro"
  | "copilot"
  | "opencode"
  | "mock"
  | "unknown";

export type RoleBadge =
  | "strategist"
  | "engineer"
  | "analyst"
  | "executor"
  | "stand-in"
  | "unknown";

export interface VendorGlyph {
  /// Single character used as the body of the placeholder mark.
  letter: string;
  /// Hue token consumed by the role band stripe.
  hue: VendorHue;
  /// Human-readable vendor name used in the accessible label.
  vendorLabel: string;
}

/// Visual state overlay applied around the glyph (state ring color
/// + optional corner mark). Tracks `WorkerState` 1:1 plus an `unknown`
/// fallback for forward-compat.
export type StateOverlay =
  | "idle"
  | "planning"
  | "executing"
  | "blocked"
  | "reviewing"
  | "done"
  | "failed"
  | "unknown";

/// Restrained motion vocabulary. One profile per state. CSS owns the
/// keyframes; `prefers-reduced-motion` zeroes out ambient animations.
export type AnimationProfile =
  | "static"
  | "breath"
  | "pulse"
  | "spin"
  | "blink"
  | "lock-in";

export interface AvatarProfile {
  avatarType: AvatarType;
  vendorGlyph: VendorGlyph;
  roleBadge: RoleBadge;
  stateOverlay: StateOverlay;
  animationProfile: AnimationProfile;
  /// Accessible label, e.g. "Claude worker · strategist · executing".
  label: string;
}

const VENDOR_GLYPHS: Record<VendorHue, VendorGlyph> = {
  claude: { letter: "C", hue: "claude", vendorLabel: "Claude" },
  codex: { letter: "X", hue: "codex", vendorLabel: "Codex" },
  gemini: { letter: "G", hue: "gemini", vendorLabel: "Gemini" },
  antigravity: { letter: "A", hue: "antigravity", vendorLabel: "Antigravity" },
  kiro: { letter: "K", hue: "kiro", vendorLabel: "Kiro" },
  copilot: { letter: "P", hue: "copilot", vendorLabel: "Copilot" },
  opencode: { letter: "O", hue: "opencode", vendorLabel: "OpenCode" },
  mock: { letter: "M", hue: "mock", vendorLabel: "Mock" },
  unknown: { letter: "?", hue: "unknown", vendorLabel: "Unknown vendor" },
};

const VENDOR_ROLES: Record<VendorHue, RoleBadge> = {
  claude: "strategist",
  codex: "engineer",
  gemini: "analyst",
  antigravity: "executor",
  kiro: "engineer",
  copilot: "engineer",
  opencode: "executor",
  mock: "stand-in",
  unknown: "unknown",
};

const STATE_OVERLAY: Record<WorkerState, StateOverlay> = {
  idle: "idle",
  planning: "planning",
  executing: "executing",
  blocked: "blocked",
  reviewing: "reviewing",
  done: "done",
  failed: "failed",
};

const STATE_ANIMATION: Record<WorkerState, AnimationProfile> = {
  idle: "breath",
  planning: "pulse",
  executing: "spin",
  blocked: "blink",
  reviewing: "pulse",
  done: "lock-in",
  failed: "static",
};

const STATE_LABEL_TOKEN: Record<StateOverlay, string> = {
  idle: "idle",
  planning: "planning",
  executing: "executing",
  blocked: "blocked",
  reviewing: "reviewing",
  done: "done",
  failed: "failed",
  unknown: "unknown state",
};

function hueFromVendor(vendor: Vendor | string): VendorHue {
  switch (vendor) {
    case "claude":
    case "codex":
    case "gemini":
    case "antigravity":
    case "kiro":
    case "copilot":
    case "opencode":
    case "mock":
      return vendor;
    default:
      return "unknown";
  }
}

export function getVendorGlyph(vendor: Vendor | string): VendorGlyph {
  return VENDOR_GLYPHS[hueFromVendor(vendor)];
}

export function getRoleBadge(vendor: Vendor | string): RoleBadge {
  return VENDOR_ROLES[hueFromVendor(vendor)];
}

export function getStateOverlay(state: WorkerState | string): StateOverlay {
  return (STATE_OVERLAY as Record<string, StateOverlay>)[state] ?? "unknown";
}

export function getAnimationProfile(
  state: WorkerState | string,
): AnimationProfile {
  return (STATE_ANIMATION as Record<string, AnimationProfile>)[state] ?? "static";
}

export function formatAvatarLabel(
  glyph: VendorGlyph,
  role: RoleBadge,
  overlay: StateOverlay,
): string {
  const stateToken = STATE_LABEL_TOKEN[overlay];
  return `${glyph.vendorLabel} worker · ${role} · ${stateToken}`;
}

export function getAvatarType(vendor: Vendor | string): AvatarType {
  return PORTRAIT_VENDORS.has(hueFromVendor(vendor)) ? "portrait" : "placeholder";
}

export function getAvatarProfile(
  vendor: Vendor | string,
  state: WorkerState | string,
): AvatarProfile {
  const vendorGlyph = getVendorGlyph(vendor);
  const roleBadge = getRoleBadge(vendor);
  const stateOverlay = getStateOverlay(state);
  const animationProfile = getAnimationProfile(state);
  return {
    avatarType: getAvatarType(vendor),
    vendorGlyph,
    roleBadge,
    stateOverlay,
    animationProfile,
    label: formatAvatarLabel(vendorGlyph, roleBadge, stateOverlay),
  };
}
