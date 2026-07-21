// S10 — residual-risk band badge. Pure render: takes a band
// classification (Low / Medium / High) from S9's CompletionVerdict
// and renders a coloured pill. The color tokens are declared in
// index.css; see also the lexicon entry for "residual risk".

import type { RiskBand } from "../bindings";

interface RiskBandBadgeProps {
  /** S9's RiskBand enum, wire-form. Defensive: unknown values
   *  render the medium pill (safer than throwing in a render
   *  path; the lint will catch it). */
  band: RiskBand;
  /** Optional className for callsite layout. */
  className?: string;
}

const LABEL: Record<RiskBand, string> = {
  low: "Low risk",
  medium: "Medium risk",
  high: "High risk",
};

export default function RiskBandBadge({ band, className }: RiskBandBadgeProps) {
  const normalized: RiskBand =
    band === "low" || band === "medium" || band === "high" ? band : "medium";
  const label = LABEL[normalized];
  return (
    <span
      className={[
        "risk-band-badge",
        `risk-band-badge--${normalized}`,
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
      aria-label={label}
      role="status"
    >
      <span className="risk-band-dot risk-band-badge-dot" aria-hidden="true" />
      {label}
    </span>
  );
}
