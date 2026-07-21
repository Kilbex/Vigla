import { memo } from "react";

interface HudMarkProps {
  /**
   * Pixel size of the rendered SVG (the mark is square). Three call
   * sites currently use 48 (inbox empty state) and 160 (ops-room launch
   * empty state). The shape is identical; only the size and opacity scale.
   */
  size: number;
  /**
   * Optional className applied to the outer <svg>. Used by call sites
   * that want to scope CSS (e.g. `.inbox-overview-empty__reticle`).
   */
  className?: string;
}

/**
 * Shared decorative reticle. Renders concentric rings + cardinal
 * cross marks + a small center dot in pure SVG. Used as an empty-state
 * mark across the app. Stroke widths and opacities are tuned to scale
 * with `size` so the smaller variants stay readable and the larger
 * variants stay subtle.
 *
 * This is purely decorative — no aria-label is set and `aria-hidden`
 * is true. Callers that want the mark to be announced should wrap it
 * in a labelled element.
 */
function HudMarkInner({ size, className }: HudMarkProps) {
  // Coordinate space is fixed at 48; CSS scales via width/height.
  const s = 48;
  const cx = s / 2;
  const cy = s / 2;
  const outerR = s / 2 - 4;
  const innerR = s / 4;
  // Tick marks emerge from `tickStart` to `tickEnd` from each edge
  // along the cardinal axes. Numbers chosen to match the prior
  // InboxOverview reticle so the visual is consistent.
  const tickInset = 2;
  const tickLen = 6;
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${s} ${s}`}
      aria-hidden
      focusable="false"
      className={className}
    >
      <circle cx={cx} cy={cy} r={outerR} fill="none" stroke="currentColor" strokeWidth="0.8" opacity="0.5" />
      <circle cx={cx} cy={cy} r={innerR} fill="none" stroke="currentColor" strokeWidth="0.8" opacity="0.7" />
      <line x1={cx} y1={tickInset}                x2={cx} y2={tickInset + tickLen}                 stroke="currentColor" strokeWidth="0.8" />
      <line x1={cx} y1={s - tickInset - tickLen}  x2={cx} y2={s - tickInset}                       stroke="currentColor" strokeWidth="0.8" />
      <line x1={tickInset}                y1={cy} x2={tickInset + tickLen} y2={cy}                 stroke="currentColor" strokeWidth="0.8" />
      <line x1={s - tickInset - tickLen}  y1={cy} x2={s - tickInset}       y2={cy}                 stroke="currentColor" strokeWidth="0.8" />
      <circle cx={cx} cy={cy} r="2" fill="currentColor" />
    </svg>
  );
}

export const HudMark = memo(HudMarkInner);
export default HudMark;
