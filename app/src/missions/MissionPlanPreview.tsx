// QC-2 — Plan review workspace.
//
// Appears while a proposed plan is awaiting approval. The execution map is
// the primary review surface; the full textual plan remains available as an
// accessible, progressively disclosed alternative. Model-authored text is
// sanitized before it reaches either projection.

import { useEffect, useMemo, useState } from "react";
import { commands } from "../bindings";
import {
  PLAN_CONTENT_LIMITS,
  sanitizePlanDetail,
  sanitizePlanLabel,
} from "./plan-content";
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

/** Mirrors the orchestrator's canonical envelope order. */
function firstExceedsBound(envelopeFit: EnvelopeFit): string {
  for (const key of [
    "scope",
    "reversibility",
    "risk",
    "quality",
  ] as const) {
    if (envelopeFit[key]?.fit === "exceeds") return BOUND_LABEL[key];
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

  const safeTitle = sanitizePlanLabel(mission.spec.title) || "Untitled mission";
  const safeObjective = sanitizePlanDetail(mission.spec.objective);
  const safeStatusLine = sanitizePlanDetail(mission.statusLine);
  const safeOverview = sanitizePlanDetail(mission.planOverview);
  const visibleTasks = useMemo(() => {
    let remainingDependencyInputs = PLAN_CONTENT_LIMITS.dependencyInputs;
    return mission.tasks
      .slice(0, PLAN_CONTENT_LIMITS.tasks)
      .map((task, index) => {
        const rawScopePaths = Array.isArray(task.scopePaths)
          ? task.scopePaths
          : [];
        const rawDependencies = Array.isArray(task.dependsOn)
          ? task.dependsOn
          : [];
        const boundedDependencies = rawDependencies.slice(
          0,
          remainingDependencyInputs,
        );
        remainingDependencyInputs -= boundedDependencies.length;
        return {
          index: task.index,
          title: sanitizePlanLabel(task.title) || `Task ${index + 1}`,
          description: sanitizePlanDetail(task.description),
          role: sanitizePlanLabel(task.role),
          dependsOn: Array.from(
            new Set(
              boundedDependencies.filter(
                (dependency) =>
                  Number.isSafeInteger(dependency) && dependency >= 0,
              ),
            ),
          ),
          omittedDependencyCount: Math.max(
            0,
            rawDependencies.length - boundedDependencies.length,
          ),
          criteriaSummary: sanitizePlanDetail(task.criteriaSummary),
          scopePaths: rawScopePaths
            .slice(0, PLAN_CONTENT_LIMITS.scopePathsPerTask)
            .map(sanitizePlanLabel)
            .filter(Boolean),
          omittedScopePathCount: Math.max(
            0,
            rawScopePaths.length - PLAN_CONTENT_LIMITS.scopePathsPerTask,
          ),
        };
      });
  }, [mission.tasks]);
  const visibleTaskTitles = useMemo(
    () => new Map(visibleTasks.map((task) => [task.index, task.title])),
    [visibleTasks],
  );
  const visibleTechStack = useMemo(
    () =>
      (mission.planTechStack ?? [])
        .slice(0, PLAN_CONTENT_LIMITS.techItems)
        .map((item) => ({
          ...item,
          layer: sanitizePlanLabel(item.layer) || "Layer",
          choice: sanitizePlanLabel(item.choice) || "Choice",
          rationale: sanitizePlanDetail(item.rationale),
        })),
    [mission.planTechStack],
  );
  const planSummary = [
    `${mission.tasks.length} task${mission.tasks.length === 1 ? "" : "s"}`,
    `Draft ${mission.planGeneration + 1}`,
  ];
  const envelopeSummary = summarizeEnvelopeFit(mission.planEnvelopeFit);
  const omittedDetailCount =
    Math.max(0, mission.tasks.length - visibleTasks.length) +
    Math.max(
      0,
      (mission.planTechStack?.length ?? 0) - visibleTechStack.length,
    ) +
    visibleTasks.reduce(
      (total, task) =>
        total + task.omittedScopePathCount + task.omittedDependencyCount,
      0,
    );

  // Stable props keep the Dagre projection from recomputing on parent renders.
  const mindMapSpec = useMemo(
    () => ({ title: safeTitle, objective: safeObjective }),
    [safeObjective, safeTitle],
  );
  const mindMapPlan = useMemo(
    () => ({
      tasks: mission.tasks
        .slice(0, PLAN_CONTENT_LIMITS.tasks)
        .map((task) => ({
          index: task.index,
          title: task.title,
          description: task.description ?? null,
          depends_on: task.dependsOn ?? [],
          role: task.role,
          criteria_summary: task.criteriaSummary,
          scope_paths: task.scopePaths ?? [],
        })),
      source_task_count: mission.tasks.length,
      generation: mission.planGeneration,
      overview: mission.planOverview,
      tech_stack: (mission.planTechStack ?? []).slice(
        0,
        PLAN_CONTENT_LIMITS.techItems,
      ),
      source_tech_stack_count: mission.planTechStack?.length ?? 0,
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
      if (result.status === "error") setError(result.error);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
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
      setHint("");
      setRegenOpen(false);
      setRegeneratingGeneration(mission.planGeneration);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setSubmitting(null);
    }
  };

  return (
    <div className="mission-plan-preview">
      <header className="mission-plan-preview__header">
        <div className="mission-plan-preview__heading">
          <span className="mission-plan-preview__chrome">Plan review</span>
          <h2 className="mission-plan-preview__title">{safeTitle}</h2>
          <div
            className="mission-plan-preview__meta"
            aria-label={planSummary.join(", ")}
          >
            {planSummary.map((item) => (
              <span key={item}>{item}</span>
            ))}
            {envelopeSummary ? (
              <span
                className={`mission-plan-preview__fit mission-plan-preview__fit--${envelopeSummary.tone}`}
              >
                <span className="mission-plan-preview__fit-dot" aria-hidden />
                {envelopeSummary.label}
              </span>
            ) : null}
          </div>
        </div>
        {elapsed ? <span className="mission-active__elapsed">{elapsed}</span> : null}
      </header>

      {envelopeTrip ? (
        <div
          className="mission-plan-preview__banner mission-plan-preview__banner--envelope"
          role="alert"
        >
          <EnvelopeAlertIcon />
          <span>
            {envelopeTrip} exceeds the mission envelope. Review is required
            before workers can start.
          </span>
        </div>
      ) : null}

      <PlanEnvelopePanel envelopeFit={mission.planEnvelopeFit} />

      <section
        className="mission-plan-preview__map"
        aria-labelledby="plan-map-title"
      >
        <div className="mission-plan-preview__map-header">
          <div>
            <h3
              id="plan-map-title"
              className="mission-plan-preview__section-title"
            >
              Execution map
            </h3>
            <p>Drag to pan. Scroll to zoom.</p>
          </div>
        </div>
        <PlanMindMap spec={mindMapSpec} plan={mindMapPlan} height={360} />
      </section>

      <details className="mission-plan-preview__details">
        <summary className="mission-plan-preview__details-summary">
          <span>Plan details</span>
          <span className="mission-plan-preview__details-count">
            {visibleTasks.length} task{visibleTasks.length === 1 ? "" : "s"}
            {" · "}
            {visibleTechStack.length} stack choice
            {visibleTechStack.length === 1 ? "" : "s"}
          </span>
          <DisclosureIcon />
        </summary>
        <div className="mission-plan-preview__details-body">
          {safeObjective ? (
            <section className="mission-plan-preview__overview">
              <h3 className="mission-plan-preview__section-title">Objective</h3>
              <p>{safeObjective}</p>
            </section>
          ) : null}

          {safeOverview ? (
            <section className="mission-plan-preview__overview">
              <h3 className="mission-plan-preview__section-title">Overview</h3>
              <p>{safeOverview}</p>
            </section>
          ) : null}

          {visibleTechStack.length > 0 ? (
            <section className="mission-plan-preview__tech-stack">
              <h3 className="mission-plan-preview__section-title">Tech stack</h3>
              <ul>
                {visibleTechStack.map((item, index) => (
                  <li key={`${item.layer}-${index}`}>
                    <div className="mission-plan-preview__tech-heading">
                      <span className="mission-plan-preview__tech-layer">
                        {item.layer}
                      </span>
                      <span>{item.choice}</span>
                      {item.is_new ? (
                        <span className="mission-plan-preview__tech-new">New</span>
                      ) : null}
                    </div>
                    {item.rationale ? <p>{item.rationale}</p> : null}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}

          <section className="mission-plan-preview__tasks">
            <h3 className="mission-plan-preview__section-title">Tasks</h3>
            {visibleTasks.length === 0 ? (
              <p className="mission-review__faint">No tasks proposed.</p>
            ) : (
              <ol className="mission-plan-preview__list">
                {visibleTasks.map((task, index) => (
                  <li
                    key={`${task.index}-${index}`}
                    className="mission-plan-preview__task"
                  >
                    <span
                      className="mission-plan-preview__task-index"
                      aria-hidden
                    >
                      {String(index + 1).padStart(2, "0")}
                    </span>
                    <div className="mission-plan-preview__task-content">
                      <h4 className="mission-plan-preview__task-title">
                        {task.title}
                      </h4>
                      <p className="mission-plan-preview__task-description">
                        {task.description || "No description provided."}
                      </p>
                      <dl className="mission-plan-preview__task-facts">
                        <div>
                          <dt>Role</dt>
                          <dd>{task.role || "Not specified"}</dd>
                        </div>
                        <div>
                          <dt>Depends on</dt>
                          <dd>
                            {task.dependsOn.length > 0
                              ? task.dependsOn
                                  .map(
                                    (dependency) =>
                                      visibleTaskTitles.get(dependency) ??
                                      `Task ${dependency + 1}`,
                                  )
                                  .join(", ")
                              : "None"}
                            {task.omittedDependencyCount > 0
                              ? `; ${task.omittedDependencyCount} more omitted`
                              : ""}
                          </dd>
                        </div>
                        <div>
                          <dt>Acceptance criteria</dt>
                          <dd>{task.criteriaSummary || "Not specified"}</dd>
                        </div>
                        <div>
                          <dt>Scope</dt>
                          <dd>
                            {task.scopePaths.length > 0
                              ? task.scopePaths.join(", ")
                              : "Not specified"}
                            {task.omittedScopePathCount > 0
                              ? `; ${task.omittedScopePathCount} more omitted`
                              : ""}
                          </dd>
                        </div>
                      </dl>
                    </div>
                  </li>
                ))}
              </ol>
            )}
          </section>

          {omittedDetailCount > 0 ? (
            <p className="mission-plan-preview__omission" role="note">
              {omittedDetailCount} additional plan item
              {omittedDetailCount === 1 ? " is" : "s are"} omitted from this
              preview.
            </p>
          ) : null}
        </div>
      </details>

      <div className="mission-plan-preview__decision">
        {safeStatusLine ? (
          <p className="mission-plan-preview__status" aria-live="polite">
            {safeStatusLine}
          </p>
        ) : null}

        {error ? (
          <div className="mission-form__error" role="alert">
            {sanitizePlanDetail(error)}
          </div>
        ) : null}

        {showRevisionNotice ? (
          <div className="mission-plan-preview__revision-notice" role="note">
            Regenerated {mission.planGeneration} times. If the plan still misses
            the objective, reject it and refine the mission brief.
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
              className="mission-form__button mission-form__button--danger"
              onClick={() => setRejectOpen(true)}
              disabled={disabled}
            >
              Reject Plan
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
              className="mission-form__button mission-form__button--primary"
              onClick={runConfirm}
              disabled={disabled}
            >
              {submitting === "confirm" ? "Approving…" : "Approve Plan"}
            </button>
          </div>
        ) : (
          <div className="mission-plan-preview__regen">
            <label className="mission-form__field">
              <span className="mission-form__label">What should change?</span>
              <textarea
                className="mission-form__textarea"
                value={hint}
                onChange={(event) => setHint(event.target.value)}
                placeholder="Start with tests, then split the callback work."
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
                  : "Regenerate Without Feedback"}
              </button>
              <button
                type="button"
                className="mission-form__button mission-form__button--primary"
                onClick={() => runRegenerate(hint.trim())}
                disabled={disabled || hint.trim().length === 0}
              >
                {submitting === "regenerate" && hint.trim().length > 0
                  ? "Sending…"
                  : "Regenerate With Feedback"}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function summarizeEnvelopeFit(
  envelopeFit: EnvelopeFit | null | undefined,
): { tone: "within" | "near" | "exceeds"; label: string } | null {
  if (!envelopeFit) return null;
  const fits = Object.values(envelopeFit).map((bound) => bound.fit);
  const exceeds = fits.filter((fit) => fit === "exceeds").length;
  if (exceeds > 0) {
    return {
      tone: "exceeds",
      label: `${exceeds} bound${exceeds === 1 ? "" : "s"} exceeded`,
    };
  }
  const near = fits.filter((fit) => fit === "near_limit").length;
  if (near > 0) {
    return {
      tone: "near",
      label: `${near} bound${near === 1 ? "" : "s"} near limit`,
    };
  }
  return { tone: "within", label: "Within envelope" };
}

function EnvelopeAlertIcon() {
  return (
    <svg
      className="mission-plan-preview__alert-icon"
      aria-hidden="true"
      viewBox="0 0 20 20"
      focusable="false"
    >
      <path d="M10 2.8 18 17H2L10 2.8Z" />
      <path d="M10 7v4.8M10 14.5v.1" />
    </svg>
  );
}

function DisclosureIcon() {
  return (
    <svg
      className="mission-plan-preview__disclosure-icon"
      aria-hidden="true"
      viewBox="0 0 16 16"
      focusable="false"
    >
      <path d="m4.5 6 3.5 3.5L11.5 6" />
    </svg>
  );
}
