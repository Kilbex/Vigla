import RevertButton from "../inbox/RevertButton";
import CleanupButton from "../inbox/CleanupButton";
import RiskBandBadge from "../inbox/RiskBandBadge";
import type { RevertOutcomeDto } from "../bindings";
import {
  formatTestsForMission,
  finalRollbackTagForMission,
  isTestsRowFallback,
} from "./trustSnapshot";
import type { ActiveMission, MissionLifecycle } from "./types";

interface Props {
  mission: ActiveMission;
  elapsed?: string;
  onDone: () => void;
  onViewChanges: () => void;
  onReverted: (outcome: RevertOutcomeDto) => void;
}

export default function MissionTerminalOutcome({
  mission,
  elapsed,
  onDone,
  onViewChanges,
  onReverted,
}: Props) {
  const tests = formatTestsForMission(mission);
  const integratedCount = mission.tasks.filter(
    (t) => t.status === "integrated",
  ).length;
  const unresolvedCount = mission.verdict?.unresolved_issues.length ?? 0;
  const canRevert = mission.lifecycle === "merged";
  const canCleanUp = mission.lifecycle === "aborted";
  const rollbackAnchor = finalRollbackTagForMission(
    mission.id,
    mission.spec.target_ref,
  );
  const riskBand = mission.verdict?.residual_risk ?? null;
  const auditOverall =
    typeof mission.audit?.overall === "number"
      ? mission.audit.overall.toFixed(2)
      : "n/a";

  return (
    <div className="mission-terminal">
      <header className="mission-terminal__header">
        <div>
          <span className="mission-terminal__eyebrow">Mission resolved</span>
          <h2 className="mission-terminal__title">
            {terminalHeadline(mission.lifecycle)}
          </h2>
          <p className="mission-terminal__mission-title">{mission.spec.title}</p>
        </div>
        {elapsed ? <span className="mission-active__elapsed">{elapsed}</span> : null}
      </header>

      <p className="mission-terminal__summary">
        {terminalDetail(mission)}
      </p>

      <dl className="mission-terminal__score-grid" aria-label="Mission verdict">
        <div className="mission-terminal__score">
          <dt>Audit</dt>
          <dd>{auditOverall}</dd>
        </div>
        <div className="mission-terminal__score">
          <dt>Risk</dt>
          <dd>{riskBand ? <RiskBandBadge band={riskBand} /> : "n/a"}</dd>
        </div>
        <div className="mission-terminal__score">
          <dt>Tests</dt>
          <dd className={isTestsRowFallback(tests) ? "mission-review__faint" : ""}>
            {tests}
          </dd>
        </div>
        <div className="mission-terminal__score">
          <dt>Diffstat</dt>
          <dd>
            {mission.filesChanged} file{mission.filesChanged === 1 ? "" : "s"}
          </dd>
        </div>
        <div className="mission-terminal__score">
          <dt>Tasks</dt>
          <dd>
            {integratedCount}/{mission.tasks.length} integrated
          </dd>
        </div>
        <div className="mission-terminal__score">
          <dt>Unresolved</dt>
          <dd
            className={
              unresolvedCount > 0 ? "mission-terminal__score-alert" : undefined
            }
          >
            {unresolvedCount}
          </dd>
        </div>
      </dl>

      {mission.verdict ? (
        <p className="mission-terminal__recommendation">
          Recommendation: {recommendationLabel(mission.verdict.recommendation)}
        </p>
      ) : null}

      <footer className="mission-terminal__actions">
        <button
          type="button"
          className="mission-form__button mission-form__button--secondary"
          onClick={onViewChanges}
        >
          View changes
        </button>
        {canRevert ? (
          <RevertButton
            missionId={mission.id}
            rollbackAnchor={rollbackAnchor}
            onReverted={onReverted}
          />
        ) : null}
        {canCleanUp ? <CleanupButton missionId={mission.id} /> : null}
        <button
          type="button"
          className="mission-form__button mission-form__button--primary"
          onClick={onDone}
        >
          Done
        </button>
      </footer>
    </div>
  );
}

function terminalHeadline(lifecycle: MissionLifecycle): string {
  switch (lifecycle) {
    case "merged":
      return "Merged";
    case "reverted":
      return "Reverted";
    case "discarded":
      return "Discarded";
    case "extended":
      return "Extension requested";
    case "aborted":
      return "Aborted";
    default:
      return "Mission";
  }
}

function terminalDetail(mission: ActiveMission): string {
  if (mission.lifecycle === "aborted" && mission.abortReason) {
    return `Reason: ${mission.abortReason}`;
  }
  if (mission.lifecycle === "merged") {
    return mission.completionSummary ?? "Mission output merged into the target branch.";
  }
  if (mission.lifecycle === "reverted") {
    return mission.restoredSha
      ? `Merged changes were reverted. Restored SHA: ${mission.restoredSha}.`
      : "Merged changes were reverted on the target branch.";
  }
  if (mission.lifecycle === "discarded") {
    return "Mission output discarded; no changes kept.";
  }
  if (mission.lifecycle === "extended") {
    return "Legacy extension request recorded; no continuation was scheduled.";
  }
  return mission.statusLine;
}

function recommendationLabel(
  recommendation: NonNullable<ActiveMission["verdict"]>["recommendation"],
): string {
  const kind = recommendation.kind;
  if (kind === "accept") return "accept";
  if (kind === "extend") return "extend";
  if (kind === "scrub") return "scrub";
  return kind;
}
