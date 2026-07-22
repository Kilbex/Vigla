import type { BoundFit, EnvelopeFit } from "./types";
import { sanitizePlanDetail } from "./plan-content";

interface Props {
  envelopeFit: EnvelopeFit | null | undefined;
}

const BOUNDS: ReadonlyArray<{ key: keyof EnvelopeFit; label: string }> = [
  { key: "scope", label: "Scope" },
  { key: "reversibility", label: "Reversibility" },
  { key: "risk", label: "Risk" },
  { key: "quality", label: "Quality" },
];

const FIT_LABEL: Record<BoundFit["fit"], string> = {
  within: "Within",
  near_limit: "Near limit",
  exceeds: "Exceeds",
};

/**
 * QC-3 — Four-row envelope-fit summary.
 *
 * Renders Scope / Reversibility / Risk / Quality with a status icon,
 * label, and the supervisor's sanitized free-form note as both inline text
 * and a tooltip (for truncated notes). Returns null when
 * `envelopeFit` is absent so callers can pass `mission?.planEnvelopeFit`
 * unconditionally.
 */
export default function PlanEnvelopePanel({ envelopeFit }: Props) {
  if (!envelopeFit) return null;
  return (
    <div className="plan-envelope" aria-label="Envelope fit" role="list">
      {BOUNDS.map(({ key, label }) => {
        const bf = envelopeFit[key];
        const note = sanitizePlanDetail(bf.note);
        return (
          <div
            key={key}
            className={`plan-envelope__row plan-envelope__row--${bf.fit}`}
            title={note || undefined}
            role="listitem"
          >
            <div className="plan-envelope__heading">
              <span className="plan-envelope__fit-icon" aria-hidden>
                <BoundStatusIcon fit={bf.fit} />
              </span>
              <span className="plan-envelope__label">{label}</span>
            </div>
            <span className="plan-envelope__value">
              <span className="plan-envelope__fit">{FIT_LABEL[bf.fit]}</span>
              {note ? (
                <span className="plan-envelope__note">{note}</span>
              ) : null}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function BoundStatusIcon({ fit }: { fit: BoundFit["fit"] }) {
  if (fit === "within") {
    return (
      <svg viewBox="0 0 16 16" focusable="false">
        <circle cx="8" cy="8" r="6" />
        <path d="m5.2 8.1 1.8 1.8 3.9-4" />
      </svg>
    );
  }
  if (fit === "near_limit") {
    return (
      <svg viewBox="0 0 16 16" focusable="false">
        <circle cx="8" cy="8" r="6" />
        <path d="M8 4.6v4.1M8 11.4v.1" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 16 16" focusable="false">
      <circle cx="8" cy="8" r="6" />
      <path d="m5.7 5.7 4.6 4.6M10.3 5.7l-4.6 4.6" />
    </svg>
  );
}
