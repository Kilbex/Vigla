// S10 — per-mission detail view. Composes RiskBandBadge +
// AuditBreakdown + UnresolvedIssuesList + SubtaskList +
// RevertButton over the active mission slice. Reads `verdict`,
// `audit`, `auditPayloadJson`, `tasks`, `lifecycle` from the
// missions store.
//
// When the surface router points at a mission whose id does not
// match the active mission (the common case for History clicks),
// the surface store carries the clicked `MissionHistoryRow` so we
// can render a degraded historical card (id + tier + audit overall
// + when + Revert) instead of the misleading "no mission selected"
// dead-end. Empty state only renders when both the active mission
// slice and the surface-store detail are null.

import { useCallback } from "react";
import { selectActiveMission, useMissionsStore } from "../missions/store";
import type { MissionLifecycle } from "../missions/types";
import {
  finalRollbackTagForMission,
  loadMissionTrustSnapshot,
  type MissionTrustSnapshot,
} from "../missions/trustSnapshot";
import AuditBreakdown from "./AuditBreakdown";
import CleanupButton from "./CleanupButton";
import { viewForUnresolvedIssue } from "./bindings-shim";
import type { MissionHistoryRow } from "./bindings-shim";
import RevertButton from "./RevertButton";
import RiskBandBadge from "./RiskBandBadge";
import RiskRowOverallCell from "./RiskRowOverallCell";
import { selectMissionDetail, useSurfaceStore } from "./router";
import SubtaskList from "./SubtaskList";
import UnresolvedIssuesList from "./UnresolvedIssuesList";

function statusBadgeClass(lc: MissionLifecycle): string {
  switch (lc) {
    case "merged":
      return "mission-inbox-status-badge--merged";
    case "reverted":
      return "mission-inbox-status-badge--discarded";
    case "discarded":
      return "mission-inbox-status-badge--discarded";
    case "aborted":
      return "mission-inbox-status-badge--aborted";
    default:
      return "mission-inbox-status-badge--running";
  }
}

function historyStatusBadgeClass(
  status: MissionHistoryRow["status"],
): string {
  switch (status) {
    case "merged":
      return "mission-inbox-status-badge--merged";
    case "discarded":
      return "mission-inbox-status-badge--discarded";
    case "aborted":
      return "mission-inbox-status-badge--aborted";
    case "audited":
      return "mission-inbox-status-badge--running";
  }
}

function fmtTimestamp(iso: string): string {
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export default function MissionInbox() {
  const mission = useMissionsStore(selectActiveMission);
  const detail = useSurfaceStore(selectMissionDetail);
  const back = useSurfaceStore((s) => s.back);
  const handleReverted = useCallback(() => back(), [back]);

  const activeMissionMatches =
    mission !== null && detail !== null && mission.id === detail.missionId;

  // Historical view: requested id ≠ active id, but we have the
  // clicked-row payload to render a degraded card.
  if (!activeMissionMatches && detail?.row) {
    const snapshot = loadMissionTrustSnapshot(detail.row.mission_id);
    if (snapshot) {
      return (
        <SnapshotMissionView
          row={detail.row}
          snapshot={snapshot}
          onBack={back}
          onReverted={handleReverted}
        />
      );
    }
    return (
      <HistoricalMissionView
        row={detail.row}
        onBack={back}
        onReverted={handleReverted}
      />
    );
  }

  // Empty state: no active mission slice AND no detail address.
  if (!mission && !detail) {
    return (
      <section className="mission-inbox" aria-label="Mission detail">
        <header className="mission-inbox-header">
          <div className="mission-inbox-header-titles">
            <h2 className="mission-inbox-title">No mission selected</h2>
          </div>
          <button
            type="button"
            className="mission-inbox-back"
            onClick={back}
            aria-label="back"
          >
            ← Back
          </button>
        </header>
        <div className="mission-inbox-empty">
          Open a mission from the inbox or history to see its details.
        </div>
      </section>
    );
  }

  // If we get here without an active mission, the surface router
  // pointed at an id with no row payload and no matching active
  // slice. Fall back to the placeholder header rather than crashing.
  if (!mission) {
    return (
      <section className="mission-inbox" aria-label="Mission detail">
        <header className="mission-inbox-header">
          <div className="mission-inbox-header-titles">
            <h2 className="mission-inbox-title">No mission selected</h2>
          </div>
          <button
            type="button"
            className="mission-inbox-back"
            onClick={back}
            aria-label="back"
          >
            ← Back
          </button>
        </header>
        <div className="mission-inbox-empty">
          Open a mission from the inbox or history to see its details.
        </div>
      </section>
    );
  }

  const verdict = mission.verdict;
  const auditPayloadJson = mission.auditPayloadJson ?? null;
  const rollbackAnchor = finalRollbackTagForMission(
    mission.id,
    mission.spec.target_ref,
  );
  const canRevert = mission.lifecycle === "merged";
  const canCleanUp = mission.lifecycle === "aborted";
  const issueViews = (verdict?.unresolved_issues ?? []).map(viewForUnresolvedIssue);

  return (
    <section className="mission-inbox" aria-label="Mission detail">
      <header className="mission-inbox-header">
        <div className="mission-inbox-header-titles">
          <h2 className="mission-inbox-title">{mission.spec.title}</h2>
          <div className="mission-inbox-subtitle">{mission.id}</div>
          <div>
            <span
              className={[
                "mission-inbox-status-badge",
                statusBadgeClass(mission.lifecycle),
              ].join(" ")}
              aria-label={`Mission status: ${mission.lifecycle}`}
            >
              {mission.lifecycle.toUpperCase()}
            </span>{" "}
            {verdict ? <RiskBandBadge band={verdict.residual_risk} /> : null}
          </div>
        </div>
        <button
          type="button"
          className="mission-inbox-back"
          onClick={back}
          aria-label="back"
        >
          ← Back
        </button>
      </header>

      <div>
        <h3 className="mission-inbox-section-title">Audit breakdown</h3>
        <AuditBreakdown
          tier={mission.audit?.tier ?? "smoke"}
          payloadJson={auditPayloadJson}
        />
      </div>
      <div>
        <h3 className="mission-inbox-section-title">Unresolved issues</h3>
        <UnresolvedIssuesList issues={issueViews} />
      </div>
      <div>
        <h3 className="mission-inbox-section-title">Subtasks</h3>
        <SubtaskList tasks={mission.tasks} />
      </div>

      {canRevert ? (
        <div className="mission-inbox-actions">
          <RevertButton
            missionId={mission.id}
            rollbackAnchor={rollbackAnchor}
            onReverted={handleReverted}
          />
        </div>
      ) : null}
      {canCleanUp ? (
        <div className="mission-inbox-actions">
          <CleanupButton missionId={mission.id} onCleaned={handleReverted} />
        </div>
      ) : null}
    </section>
  );
}

interface HistoricalMissionViewProps {
  row: MissionHistoryRow;
  onBack: () => void;
  onReverted: () => void;
}

interface SnapshotMissionViewProps extends HistoricalMissionViewProps {
  snapshot: MissionTrustSnapshot;
}

function SnapshotMissionView({
  row,
  snapshot,
  onBack,
  onReverted,
}: SnapshotMissionViewProps) {
  const issueViews = (snapshot.verdict?.unresolved_issues ?? []).map(
    viewForUnresolvedIssue,
  );
  // Only the durable History row authorizes a merged-target rollback. Local
  // snapshots are presentation data and must not fill a missing target ref.
  const targetRef = row.target_ref ?? "";
  const canRevert =
    !row.reverted &&
    row.status === "merged" &&
    targetRef.trim().length > 0 &&
    (row.repo_root?.trim().length ?? 0) > 0;
  const canCleanUp =
    row.status === "aborted" &&
    !row.artifacts_cleaned &&
    (row.repo_root?.trim().length ?? 0) > 0;
  const statusLabel = row.reverted ? "Reverted" : row.status;
  const statusClass = row.reverted
    ? "mission-inbox-status-badge--discarded"
    : historyStatusBadgeClass(row.status);

  return (
    <section
      className="mission-inbox"
      aria-label="Mission detail (historical)"
    >
      <header className="mission-inbox-header">
        <div className="mission-inbox-header-titles">
          <h2 className="mission-inbox-title">{snapshot.title}</h2>
          <div className="mission-inbox-subtitle">{snapshot.missionId}</div>
          <div className="mission-inbox-subtitle">
            {fmtTimestamp(snapshot.updatedAt)}
          </div>
          <div>
            <span
              className={["mission-inbox-status-badge", statusClass].join(" ")}
              aria-label={`Mission status: ${statusLabel}`}
            >
              {statusLabel.toUpperCase()}
            </span>{" "}
            {snapshot.verdict ? (
              <RiskBandBadge band={snapshot.verdict.residual_risk} />
            ) : null}
          </div>
        </div>
        <button
          type="button"
          className="mission-inbox-back"
          onClick={onBack}
          aria-label="back"
        >
          ← Back
        </button>
      </header>

      {snapshot.summary ? (
        <p className="mission-inbox-summary">{snapshot.summary}</p>
      ) : null}

      <div>
        <h3 className="mission-inbox-section-title">Audit breakdown</h3>
        <AuditBreakdown
          tier={snapshot.audit?.tier ?? row.tier}
          payloadJson={snapshot.auditPayloadJson}
        />
      </div>
      <div>
        <h3 className="mission-inbox-section-title">Unresolved issues</h3>
        <UnresolvedIssuesList issues={issueViews} />
      </div>
      <div>
        <h3 className="mission-inbox-section-title">Subtasks</h3>
        <SubtaskList tasks={snapshot.tasks} />
      </div>

      {canRevert ? (
        <div className="mission-inbox-actions">
          <RevertButton
            missionId={snapshot.missionId}
            rollbackAnchor={finalRollbackTagForMission(
              snapshot.missionId,
              targetRef,
            )}
            onReverted={onReverted}
          />
        </div>
      ) : null}
      {canCleanUp ? (
        <div className="mission-inbox-actions">
          <CleanupButton
            missionId={snapshot.missionId}
            onCleaned={onReverted}
          />
        </div>
      ) : null}
      {row.status === "aborted" && row.artifacts_cleaned ? (
        <p role="status">Artifacts cleaned.</p>
      ) : null}
    </section>
  );
}

/** Degraded detail view for a mission we know about only through
 *  `list_recent_missions`. Renders the id, audit-overall pill, tier,
 *  timestamp, terminal status, and (only for a durably merged row)
 *  the RevertButton. Re-uses the
 *  existing `.mission-inbox-*` chrome so the card visually matches
 *  the active-mission view. */
function HistoricalMissionView({
  row,
  onBack,
  onReverted,
}: HistoricalMissionViewProps) {
  const targetRef = row.target_ref ?? "";
  const canRevert =
    !row.reverted &&
    row.status === "merged" &&
    targetRef.trim().length > 0 &&
    (row.repo_root?.trim().length ?? 0) > 0;
  const canCleanUp =
    row.status === "aborted" &&
    !row.artifacts_cleaned &&
    (row.repo_root?.trim().length ?? 0) > 0;
  const statusLabel = row.reverted ? "Reverted" : row.status;
  const statusClass = row.reverted
    ? "mission-inbox-status-badge--discarded"
    : historyStatusBadgeClass(row.status);

  return (
    <section
      className="mission-inbox"
      aria-label="Mission detail (historical)"
    >
      <header className="mission-inbox-header">
        <div className="mission-inbox-header-titles">
          <h2 className="mission-inbox-title">{row.mission_id}</h2>
          <div className="mission-inbox-subtitle">
            {fmtTimestamp(row.created_at)}
          </div>
          <div>
            <span
              className={["mission-inbox-status-badge", statusClass].join(" ")}
              aria-label={`Mission status: ${statusLabel}`}
            >
              {statusLabel.toUpperCase()}
            </span>{" "}
            <span
              className="mission-inbox-status-badge mission-inbox-status-badge--running"
              aria-label={`Audit tier: ${row.tier}`}
            >
              {row.tier.toUpperCase()}
            </span>
          </div>
        </div>
        <button
          type="button"
          className="mission-inbox-back"
          onClick={onBack}
          aria-label="back"
        >
          ← Back
        </button>
      </header>

      <div>
        <h3 className="mission-inbox-section-title">Audit overall</h3>
        <table
          className="mission-history-table"
          aria-label="Historical audit overall"
        >
          <tbody>
            <tr className="mission-history-row">
              <RiskRowOverallCell overall={row.audit_overall} />
            </tr>
          </tbody>
        </table>
      </div>

      <div className="mission-inbox-empty mission-inbox-empty--degraded">
        Detailed verdict data was not retained for this older mission.
        Future merged missions keep their audit, risk, issue, and subtask
        trail here.
      </div>

      {canRevert ? (
        <div className="mission-inbox-actions">
          <RevertButton
            missionId={row.mission_id}
            rollbackAnchor={finalRollbackTagForMission(
              row.mission_id,
              targetRef,
            )}
            onReverted={onReverted}
          />
        </div>
      ) : null}
      {canCleanUp ? (
        <div className="mission-inbox-actions">
          <CleanupButton missionId={row.mission_id} onCleaned={onReverted} />
        </div>
      ) : null}
      {row.status === "aborted" && row.artifacts_cleaned ? (
        <p role="status">Artifacts cleaned.</p>
      ) : null}
    </section>
  );
}
