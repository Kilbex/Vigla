import type { Vendor, WorkerState } from "../bindings";
import { getAvatarProfile } from "../operations/avatar";
import type { VendorHue } from "../operations/avatar";
import claudePortrait from "../assets/avatars/claude.svg";
import codexPortrait from "../assets/avatars/codex.svg";
import geminiPortrait from "../assets/avatars/gemini.svg";

interface WorkerPortraitProps {
  vendor: Vendor;
  state: WorkerState;
}

/// Per-vendor 48×56 hand-pixelled half-body portrait. Source-of-truth:
/// `docs/character-designs/generate.py`. Rendered crisp via CSS
/// `image-rendering: pixelated`. Only the three real-vendor CLIs have a
/// portrait; everything else falls back to no-render (caller decides
/// what to do).
const PORTRAITS: Partial<Record<VendorHue, string>> = {
  claude: claudePortrait,
  codex: codexPortrait,
  gemini: geminiPortrait,
};

/// Drawer-sized portrait block. Renders the 48×56 sprite at native pixel
/// scale, framed by the operations-room state vignette + vendor underglow.
/// State semantics mirror the tile avatar — same `--state-accent` and
/// per-state CSS classes.
export default function WorkerPortrait({ vendor, state }: WorkerPortraitProps) {
  const profile = getAvatarProfile(vendor, state);
  if (profile.avatarType !== "portrait") return null;
  const src = PORTRAITS[profile.vendorGlyph.hue];
  if (!src) return null;

  const className = [
    "worker-portrait",
    "worker-portrait-frame",
    "hud-corners",
    `worker-portrait--vendor-${profile.vendorGlyph.hue}`,
    `worker-portrait--state-${profile.stateOverlay}`,
    state === "executing" ? "worker-portrait-frame--executing" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={className}
      role="img"
      aria-label={profile.label}
      data-testid="worker-portrait"
      data-vendor={profile.vendorGlyph.hue}
      data-state={profile.stateOverlay}
    >
      <img
        className="worker-portrait__sprite"
        src={src}
        alt=""
        aria-hidden
        draggable={false}
      />
      <span className="worker-portrait__vignette" aria-hidden />
    </div>
  );
}
