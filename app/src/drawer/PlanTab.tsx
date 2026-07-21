import PlanEnvelopePanel from "../missions/PlanEnvelopePanel";
import PlanMindMap from "../missions/PlanMindMap";
import type { ActiveMission } from "../missions/types";

interface Props {
  mission: ActiveMission;
}

/**
 * QC-3 — Drawer Plan tab. Read-only mirror of MissionPlanPreview's
 * mind map + envelope panel, surfaced inside the worker drawer so a
 * user inspecting a running worker can pull up the plan context
 * without leaving the drawer. No actions — Reject / Regenerate /
 * Start mission only appear on the pre-execution plan-approval
 * surface (MissionPlanPreview).
 */
export default function PlanTab({ mission }: Props) {
  return (
    <div className="plan-tab" data-testid="drawer-plan-tab">
      <PlanMindMap
        spec={{
          title: mission.spec.title,
          objective: mission.spec.objective ?? "",
        }}
        plan={{
          tasks: mission.tasks.map((t) => ({
            index: t.index,
            title: t.title,
            depends_on: t.dependsOn ?? [],
          })),
          generation: mission.planGeneration,
          overview: mission.planOverview,
          tech_stack: mission.planTechStack,
          envelope_fit: mission.planEnvelopeFit,
        }}
        height={360}
      />
      <PlanEnvelopePanel envelopeFit={mission.planEnvelopeFit} />
      {mission.planOverview ? (
        <section className="plan-tab__overview">
          <h3 className="plan-tab__section-title">Overview</h3>
          <p>{mission.planOverview}</p>
        </section>
      ) : null}
      {mission.planTechStack && mission.planTechStack.length > 0 ? (
        <section className="plan-tab__tech-stack">
          <h3 className="plan-tab__section-title">Tech stack</h3>
          <ul>
            {mission.planTechStack.map((t, i) => (
              <li key={`${t.layer}-${i}`}>
                {t.layer} · {t.choice}
                {t.is_new ? (
                  <span className="plan-tab__tech-new"> [new]</span>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
