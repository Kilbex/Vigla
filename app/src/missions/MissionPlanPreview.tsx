// QC-2 — Plan Preview screen.
//
// Appears in the overlay when lifecycle === "pending_plan_approval"
// (i.e. the mission was started with confirm_plan: true OR the
// supervisor's envelope_fit forced a pause in Direct mode). Shows
// the proposed task list and four actions: Start mission, Regenerate
// (with optional hint), Reject (QC-3), Cancel-of-regenerate. Read-
// only — the user doesn't edit tasks here; that would push them
// from "manager" to "implementer."

import { useEffect, useMemo, useState } from "react";
import { commands } from "../bindings";
import PlanEnvelopePanel from "./PlanEnvelopePanel";
import PlanMindMap from "./PlanMindMap";
import PlanRejectForm from "./PlanRejectForm";
import type { ActiveMission, EnvelopeFit } from "./types";

interface Props {
  mission: ActiveMission;
  elapsed?: string;
}

type Submitting = "confirm" | "regenerate" | null;

const BOUND_LABEL: Record<keyof EnvelopeFit, string> = {
  scope: "Scope",
  reversibility: "Reversibility",
  risk: "Risk",
  quality: "Quality",
};

/** QC-3: name of the first bound in canonical order whose fit is
 *  Exceeds. Mirrors the orchestrator's `EnvelopeFit::exceeded`
 *  ordering so the FE banner and the BE gate agree on the trip. */
function firstExceedsBound(ef: EnvelopeFit): string {
  for (const k of ["scope", "reversibility", "risk", "quality"] as const) {
    if (ef[k]?.fit === "exceeds") return BOUND_LABEL[k];
  }
  return "";
}

export default function MissionPlanPreview({ mission, elapsed }: Props) {
  const [submitting, setSubmitting] = useState<Submitting>(null);
  const [error, setError] = useState<string | null>(null);
  const [regenOpen, setRegenOpen] = useState(false);
  const [rejectOpen, setRejectOpen] = useState(false);
  const [hint, setHint] = useState("");
  const [regeneratingGeneration, setRegeneratingGeneration] = useState<
    number | null
  >(null);

  useEffect(() => {
    if (
      regeneratingGeneration !== null &&
      mission.planGeneration !== regeneratingGeneration
    ) {
      setRegeneratingGeneration(null);
    }
  }, [mission.planGeneration, regeneratingGeneration]);

  const disabled = submitting !== null || regeneratingGeneration !== null;
  const envelopeTrip =
    mission.planEnvelopeFit && firstExceedsBound(mission.planEnvelopeFit);
  const showRevisionNotice =
    mission.planGeneration >= 3 && !regenOpen && !rejectOpen;

  // Stabilize the PlanMindMap props so its dagre layout (memoized on
  // [spec, plan]) doesn't recompute on every parent render — previously these
  // were inline object literals with a fresh identity each render (FE-6).
  const mindMapSpec = useMemo(
    () => ({
      title: mission.spec.title,
      objective: mission.spec.objective ?? "",
    }),
    [mission.spec.title, mission.spec.objective],
  );
  const mindMapPlan = useMemo(
    () => ({
      tasks: mission.tasks.map((t) => ({
        index: t.index,
        title: t.title,
        description: t.description ?? null,
        depends_on: t.dependsOn ?? [],
        role: t.role,
        criteria_summary: t.criteriaSummary,
        scope_paths: t.scopePaths ?? [],
      })),
      generation: mission.planGeneration,
      overview: mission.planOverview,
      tech_stack: mission.planTechStack,
      envelope_fit: mission.planEnvelopeFit,
    }),
    [
      mission.tasks,
      mission.planGeneration,
      mission.planOverview,
      mission.planTechStack,
      mission.planEnvelopeFit,
    ],
  );

  const runConfirm = async () => {
    setSubmitting("confirm");
    setError(null);
    try {
      const result = await commands.confirmPlan(mission.planGeneration);
      if (result.status === "error") {
        setError(result.error);
        return;
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(null);
    }
  };

  const runRegenerate = async (withHint: string | null) => {
    setSubmitting("regenerate");
    setError(null);
    try {
      const result = await commands.regeneratePlan(
        mission.planGeneration,
        withHint,
      );
      if (result.status === "error") {
        setError(result.error);
        return;
      }
      // On success, leave the regen panel open so the user can see the
      // "Regenerating plan…" status line; the next plan.proposed event
      // will replace the displayed tasks and we can collapse then.
      setHint("");
      setRegenOpen(false);
      setRegeneratingGeneration(mission.planGeneration);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(null);
    }
  };

  return (
    <div className="mission-plan-preview">
      {envelopeTrip ? (
        <div
          className="mission-plan-preview__banner mission-plan-preview__banner--envelope"
          role="alert"
        >
          ⚠ Plan exceeds the {envelopeTrip} bound — review required even in
          Direct mode.
        </div>
      ) : null}

      <header className="mission-plan-preview__header">
        <h2 className="mission-plan-preview__title">
          <span className="mission-plan-preview__chrome" aria-hidden>Plan proposed</span>
          {mission.spec.title}
        </h2>
        {elapsed && <span className="mission-active__elapsed">{elapsed}</span>}
      </header>

      <p className="mission-plan-preview__hint">
        The team proposed this plan. Review and approve to start workers,
        regenerate if the plan needs adjusting, or reject to abort the
        mission.
      </p>

      <PlanEnvelopePanel envelopeFit={mission.planEnvelopeFit} />

      <p className="mission-plan-preview__status" aria-live="polite">
        {mission.statusLine}
      </p>

      {error && <div className="mission-form__error">{error}</div>}

      {showRevisionNotice ? (
        <div className="mission-plan-preview__revision-notice" role="note">
          Heads up — this plan has been regenerated {mission.planGeneration}{" "}
          times. Consider rejecting and starting over with a sharper objective
          if the iterations aren't converging.
        </div>
      ) : null}

      {rejectOpen ? (
        <PlanRejectForm
          generation={mission.planGeneration}
          onClose={() => setRejectOpen(false)}
        />
      ) : !regenOpen ? (
        <div className="mission-review__actions mission-plan-preview__actions">
          <button
            type="button"
            className="mission-form__button mission-form__button--danger hud-abort"
            onClick={() => setRejectOpen(true)}
            disabled={disabled}
          >
            Abort run
          </button>
          <button
            type="button"
            className="mission-form__button mission-form__button--secondary"
            onClick={() => setRegenOpen(true)}
            disabled={disabled}
          >
            {submitting === "regenerate" ? "Regenerating…" : "Regenerate"}
          </button>
          <button
            type="button"
            className="mission-form__button mission-form__button--primary hud-engage"
            onClick={runConfirm}
            disabled={disabled}
          >
            {submitting === "confirm" ? "Approving…" : "Approve plan"}
          </button>
        </div>
      ) : (
        <div className="mission-plan-preview__regen">
          <label className="mission-form__field">
            <span className="mission-form__label">
              What should be different? (optional)
            </span>
            <textarea
              className="mission-form__textarea"
              value={hint}
              onChange={(e) => setHint(e.target.value)}
              placeholder="e.g. Split task A into smaller pieces; add a test scaffold task first."
              rows={3}
              autoFocus
              disabled={disabled}
            />
          </label>
          <div className="mission-review__actions">
            <button
              type="button"
              className="mission-form__button mission-form__button--secondary"
              onClick={() => {
                setRegenOpen(false);
                setHint("");
              }}
              disabled={disabled}
            >
              Cancel
            </button>
            <button
              type="button"
              className="mission-form__button mission-form__button--secondary"
              onClick={() => runRegenerate(null)}
              disabled={disabled}
            >
              {submitting === "regenerate" && hint.trim().length === 0
                ? "Sending…"
                : "Regenerate without feedback"}
            </button>
            <button
              type="button"
              className="mission-form__button mission-form__button--primary"
              onClick={() => runRegenerate(hint.trim())}
              disabled={disabled || hint.trim().length === 0}
            >
              {submitting === "regenerate" && hint.trim().length > 0
                ? "Sending…"
                : "Regenerate with feedback"}
            </button>
          </div>
        </div>
      )}

      <PlanMindMap spec={mindMapSpec} plan={mindMapPlan} />

      {mission.planOverview ? (
        <section className="mission-plan-preview__overview">
          <h3 className="mission-plan-preview__section-title">Overview</h3>
          <p>{mission.planOverview}</p>
        </section>
      ) : null}

      {mission.planTechStack && mission.planTechStack.length > 0 ? (
        <section className="mission-plan-preview__tech-stack">
          <h3 className="mission-plan-preview__section-title">Tech stack</h3>
          <ul>
            {mission.planTechStack.map((t, i) => (
              <li key={`${t.layer}-${i}`}>
                {t.layer} · {t.choice}
                {t.is_new ? (
                  <span className="mission-plan-preview__tech-new">
                    {" "}
                    [new]
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <section className="mission-plan-preview__tasks">
        {mission.tasks.length === 0 ? (
          <p className="mission-review__faint">No tasks proposed yet.</p>
        ) : (
          <ol className="mission-plan-preview__list">
            {mission.tasks.map((t) => (
              <li key={t.index} className="mission-plan-preview__task">
                <span className="mission-plan-preview__task-glyph" aria-hidden>
                  ◇
                </span>
                <span className="mission-plan-preview__task-title">
                  {t.title}
                </span>
              </li>
            ))}
          </ol>
        )}
      </section>

    </div>
  );
}
