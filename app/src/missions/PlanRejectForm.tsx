import { useState } from "react";
import { commands } from "../bindings";

interface Props {
  generation: number;
  /** Called after a successful reject (so the overlay can close)
   *  or when the user cancels. The actual mission abort is driven
   *  by the orchestrator's PlanRejected → Aborted event stream. */
  onClose: () => void;
}

/**
 * QC-3 — Inline overlay for rejecting the proposed plan.
 *
 * Mirrors the existing regenerate form pattern: optional reason
 * textarea + Cancel / Confirm-without-reason / Confirm-with-reason
 * actions. Surfaces command errors inline. Calls
 * commands.rejectPlan(generation, trimmedReason | null); the orchestrator emits
 * PlanRejected then drives the Aborted transition so the existing
 * mission-event stream carries the new lifecycle.
 */
export default function PlanRejectForm({ generation, onClose }: Props) {
  const [reason, setReason] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (withReason: boolean) => {
    setSubmitting(true);
    setError(null);
    const payload = withReason ? reason.trim() || null : null;
    try {
      const result = await commands.rejectPlan(generation, payload);
      if (result.status === "error") {
        setError(result.error);
        return;
      }
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const trimmedLen = reason.trim().length;

  return (
    <div className="plan-reject-form" role="group" aria-label="reject plan">
      <label className="plan-reject-form__field">
        <span className="plan-reject-form__label">Why are you rejecting?</span>
        <textarea
          className="plan-reject-form__textarea"
          rows={3}
          autoFocus
          disabled={submitting}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          placeholder="optional — e.g. 'scope too broad; split logout out into its own mission'"
          aria-label="reject reason"
        />
      </label>
      {error ? (
        <div className="plan-reject-form__error" role="alert">
          {error}
        </div>
      ) : null}
      <div className="plan-reject-form__actions">
        <button
          type="button"
          className="mission-form__button mission-form__button--secondary"
          onClick={onClose}
          disabled={submitting}
        >
          Cancel
        </button>
        <button
          type="button"
          className="mission-form__button mission-form__button--secondary"
          onClick={() => submit(false)}
          disabled={submitting}
          aria-label="confirm reject without reason"
        >
          {submitting && trimmedLen === 0
            ? "Rejecting…"
            : "Confirm reject without reason"}
        </button>
        <button
          type="button"
          className="mission-form__button mission-form__button--danger"
          onClick={() => submit(true)}
          disabled={submitting || trimmedLen === 0}
          aria-label="confirm reject"
        >
          {submitting && trimmedLen > 0 ? "Rejecting…" : "Confirm reject"}
        </button>
      </div>
    </div>
  );
}
