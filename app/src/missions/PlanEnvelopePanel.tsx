import type { BoundFit, EnvelopeFit } from "./types";

interface Props {
  envelopeFit: EnvelopeFit | null | undefined;
}

const BOUNDS: ReadonlyArray<{ key: keyof EnvelopeFit; label: string }> = [
  { key: "scope", label: "Scope" },
  { key: "reversibility", label: "Reversibility" },
  { key: "risk", label: "Risk" },
  { key: "quality", label: "Quality" },
];

const GLYPH: Record<BoundFit["fit"], string> = {
  within: "●",
  near_limit: "◐",
  exceeds: "○",
};

const FIT_LABEL: Record<BoundFit["fit"], string> = {
  within: "within",
  near_limit: "near limit",
  exceeds: "exceeds",
};

/**
 * QC-3 — Four-row envelope-fit summary.
 *
 * Renders Scope / Reversibility / Risk / Quality with a glyph,
 * label, and the supervisor's free-form note as both inline text
 * and a tooltip (for truncated notes). Returns null when
 * `envelopeFit` is absent so callers can pass `mission?.planEnvelopeFit`
 * unconditionally.
 */
export default function PlanEnvelopePanel({ envelopeFit }: Props) {
  if (!envelopeFit) return null;
  return (
    <div className="plan-envelope" aria-label="envelope fit">
      {BOUNDS.map(({ key, label }) => {
        const bf = envelopeFit[key];
        return (
          <div
            key={key}
            className={`plan-envelope__row plan-envelope__row--${bf.fit}`}
            title={bf.note || undefined}
          >
            <span className="plan-envelope__glyph" aria-hidden>▸</span>
            <span className="plan-envelope__label">{label}</span>
            <span className="plan-envelope__value">
              <span className="plan-envelope__fit-icon" aria-hidden>
                {GLYPH[bf.fit]}
              </span>
              <span className="plan-envelope__fit">{FIT_LABEL[bf.fit]}</span>
              {bf.note ? (
                <span className="plan-envelope__note">{bf.note}</span>
              ) : null}
            </span>
          </div>
        );
      })}
    </div>
  );
}
