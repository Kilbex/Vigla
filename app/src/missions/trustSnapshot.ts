import type { CompletionVerdict } from "../bindings";
import type { ActiveMission, MissionLifecycle, MissionTask } from "./types";

const SNAPSHOT_KEY = "vigla.missionTrustSnapshots.v1";
const SNAPSHOT_LIMIT = 50;

export interface MissionTrustSnapshot {
  missionId: string;
  title: string;
  lifecycle: MissionLifecycle;
  startedAt: string;
  updatedAt: string;
  statusLine: string;
  summary: string | null;
  audit: { tier: string; overall: number } | null;
  auditPayloadJson: string | null;
  verdict: CompletionVerdict | null;
  testsLabel: string;
  filesChanged: number;
  changedFiles: string[];
  integratedCount: number;
  taskCount: number;
  tasks: MissionTask[];
  targetRef: string;
  rollbackAnchor: string;
  resolution: ActiveMission["resolution"];
  storedAt: string;
}

interface SnapshotStore {
  order: string[];
  byId: Record<string, MissionTrustSnapshot>;
}

export const TESTS_ROW_NO_DATA = "no test data";

export function formatTestsForMission(mission: ActiveMission): string {
  if (mission.testsPassed === true) return "passing";
  if (mission.testsPassed === false) return "FAILING";
  if (!mission.auditPayloadJson) return TESTS_ROW_NO_DATA;
  try {
    const parsed = JSON.parse(mission.auditPayloadJson) as {
      test_pass?: {
        ran?: boolean;
        passed?: number;
        failed?: number;
        skipped?: number;
      } | null;
    };
    const tp = parsed.test_pass;
    if (!tp) return TESTS_ROW_NO_DATA;
    if (tp.ran === false) return "no tests run";
    const passed = typeof tp.passed === "number" ? tp.passed : 0;
    const failed = typeof tp.failed === "number" ? tp.failed : 0;
    const skipped = typeof tp.skipped === "number" ? tp.skipped : 0;
    const base = `${passed} passed · ${failed} failed`;
    return skipped > 0 ? `${base} · ${skipped} skipped` : base;
  } catch {
    return TESTS_ROW_NO_DATA;
  }
}

export function isTestsRowFallback(value: string): boolean {
  return value === TESTS_ROW_NO_DATA;
}

export function finalRollbackTagForMission(
  missionId: string,
  targetRef: string,
): string {
  return `vigla/revert/${missionId}/before/${targetRef}`;
}

export function snapshotFromMission(mission: ActiveMission): MissionTrustSnapshot {
  const changedFiles = new Set<string>();
  for (const worker of Object.values(mission.workers)) {
    for (const file of worker.submittedFiles) changedFiles.add(file);
  }
  return {
    missionId: mission.id,
    title: mission.spec.title,
    lifecycle: mission.lifecycle,
    startedAt: mission.startedAt,
    updatedAt: mission.updatedAt,
    statusLine: mission.statusLine,
    summary: mission.completionSummary,
    audit: mission.audit,
    auditPayloadJson: mission.auditPayloadJson,
    verdict: mission.verdict,
    testsLabel: formatTestsForMission(mission),
    filesChanged: mission.filesChanged,
    changedFiles: Array.from(changedFiles).sort(),
    integratedCount: mission.tasks.filter((t) => t.status === "integrated").length,
    taskCount: mission.tasks.length,
    tasks: mission.tasks.map((t) => ({ ...t })),
    targetRef: mission.spec.target_ref,
    rollbackAnchor: finalRollbackTagForMission(
      mission.id,
      mission.spec.target_ref,
    ),
    resolution: mission.resolution,
    storedAt: new Date().toISOString(),
  };
}

export function persistMissionTrustSnapshot(mission: ActiveMission): void {
  const store = readStore();
  const snapshot = snapshotFromMission(mission);
  store.byId[mission.id] = snapshot;
  store.order = [mission.id, ...store.order.filter((id) => id !== mission.id)];
  for (const id of store.order.slice(SNAPSHOT_LIMIT)) {
    delete store.byId[id];
  }
  store.order = store.order.slice(0, SNAPSHOT_LIMIT);
  writeStore(store);
}

export function loadMissionTrustSnapshot(
  missionId: string,
): MissionTrustSnapshot | null {
  const stored = readStore().byId[missionId];
  if (!stored) return null;

  // v1 snapshots written before final-merge rollback anchors lack these two
  // fields. Preserve their detail content, but leave the anchor empty; the
  // durable History outcome remains the authority for whether Revert appears.
  const legacy = stored as MissionTrustSnapshot & { preMergeTag?: string };
  const targetRef = typeof legacy.targetRef === "string" ? legacy.targetRef : "";
  const rollbackAnchor =
    typeof legacy.rollbackAnchor === "string"
      ? legacy.rollbackAnchor
      : targetRef
        ? finalRollbackTagForMission(missionId, targetRef)
        : "";
  return { ...stored, targetRef, rollbackAnchor };
}

function readStore(): SnapshotStore {
  if (typeof window === "undefined" || !window.localStorage) {
    return { order: [], byId: {} };
  }
  try {
    const raw = window.localStorage.getItem(SNAPSHOT_KEY);
    if (!raw) return { order: [], byId: {} };
    const parsed = JSON.parse(raw) as Partial<SnapshotStore>;
    const order = Array.isArray(parsed.order) ? parsed.order : [];
    const byId =
      parsed.byId && typeof parsed.byId === "object" ? parsed.byId : {};
    return { order, byId: byId as Record<string, MissionTrustSnapshot> };
  } catch {
    return { order: [], byId: {} };
  }
}

function writeStore(store: SnapshotStore): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(SNAPSHOT_KEY, JSON.stringify(store));
  } catch {
    // Private-mode / quota failures should not break mission ingest.
  }
}
