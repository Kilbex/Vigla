import {
  Background,
  BackgroundVariant,
  ReactFlow,
  type Edge,
  type ReactFlowInstance,
  type Node,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useEffect, useMemo, useRef } from "react";
import {
  selectDependencyEdges,
  selectWorkerIds,
  useOpsStore,
} from "../store";
import DeployPanel from "../comms/DeployPanel";
import { selectActiveMission, useMissionsStore } from "../missions/store";
import type { WorkerSnapshot } from "../store/types";
import Station, { type StationNode } from "./Station";
import HudMark from "./HudMark";

const STATION_W = 280;
const STATION_H = 200;
const COL_GAP = 80;
const ROW_GAP = 60;
const COLS = 3;

const NODE_TYPES = { station: Station } as const;
const FIT_VIEW_OPTIONS = { padding: 0.18, includeHiddenNodes: true } as const;

export function layoutFor(index: number): { x: number; y: number } {
  const col = index % COLS;
  const row = Math.floor(index / COLS);
  const x = 60 + col * (STATION_W + COL_GAP);
  const y = 40 + row * (STATION_H + ROW_GAP);
  return { x, y };
}

function nodeFromSnapshot(snap: WorkerSnapshot, position: { x: number; y: number }): StationNode {
  // No `now: Date.now()` injection: that turned every event ingest
  // into "every node's `data` is fresh" so React Flow re-rendered
  // every tile on every event. Station owns its own flash ticker.
  return {
    id: snap.id,
    type: "station",
    position,
    data: snap as StationNode["data"],
    draggable: true,
    selectable: false,
  };
}

function edgeFor(
  e: { id: string; source: string; target: string; state: "pending" | "blocked" | "done" },
): Edge {
  const className = `dep-edge dep-edge--${e.state}`;
  return {
    id: e.id,
    source: e.source,
    target: e.target,
    type: "smoothstep",
    animated: e.state !== "done",
    className,
  };
}

export type StationNodeCache = Map<string, StationNode>;

/// Build the React Flow node list with per-id wrapper reuse.
///
/// The store re-allocates `workers` (`{...prev.workers}`) on every
/// event so OperationsRoom's memo invalidates every tick, but inside
/// that memo only one worker's snapshot ref actually changed
/// (`applyToOpsState` clones just `workers[wid]`, line 41-43 of
/// ops-state.ts). Naively recomputing every StationNode wrapper would
/// allocate N new objects per event and force React Flow to diff all
/// N — wasted on N-1 unchanged tiles. This function reuses the cached
/// wrapper when both the snapshot ref AND the layout position match,
/// so a 16-worker × 64-event burst allocates ~64 wrappers (one per
/// state change) plus 16 initial allocations, not 1024.
///
/// Exported (and pure) so the bench in `operations-room.bench.test.ts`
/// can drive it deterministically through a synthetic event stream.
export function computeStationNodes(
  prevCache: StationNodeCache,
  ids: readonly string[],
  workers: Readonly<Record<string, WorkerSnapshot>>,
): { nodes: Node[]; nextCache: StationNodeCache } {
  const nextCache: StationNodeCache = new Map();
  const out: Node[] = [];
  for (let idx = 0; idx < ids.length; idx++) {
    const id = ids[idx];
    const w = workers[id];
    if (!w) continue;
    const pos = layoutFor(idx);
    const cached = prevCache.get(id);
    const reuse =
      cached !== undefined &&
      cached.data === w &&
      cached.position.x === pos.x &&
      cached.position.y === pos.y;
    const node = reuse ? cached : nodeFromSnapshot(w, pos);
    out.push(node as unknown as Node);
    nextCache.set(id, node);
  }
  return { nodes: out, nextCache };
}

export default function OperationsRoom() {
  const ids = useOpsStore(selectWorkerIds);
  const workers = useOpsStore((s) => s.workers);
  const edges = useOpsStore(selectDependencyEdges);
  const missionActive = useMissionsStore(selectActiveMission) !== null;

  const cacheRef = useRef<StationNodeCache>(new Map());
  const flowRef = useRef<ReactFlowInstance | null>(null);

  const nodes: Node[] = useMemo(() => {
    const { nodes, nextCache } = computeStationNodes(
      cacheRef.current,
      ids,
      workers,
    );
    cacheRef.current = nextCache;
    return nodes;
  }, [ids, workers]);

  const edgeList = useMemo(() => edges.map(edgeFor), [edges]);

  // React Flow's `fitView` prop applies when the canvas first initializes.
  // Workers normally arrive afterward, so refit only when roster identity
  // changes; progress events keep their stable viewport and node positions.
  useEffect(() => {
    if (ids.length === 0) return;
    const timer = window.setTimeout(() => {
      void flowRef.current?.fitView(FIT_VIEW_OPTIONS);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [ids]);

  if (ids.length === 0) {
    if (!missionActive) {
      return (
        <div className="operations-room operations-room--empty operations-room--launch">
          <HudMark size={160} className="operations-room__compass" />
          <div className="operations-room__standby" aria-hidden>STANDBY</div>
          <div className="operations-room-launch">
            <DeployPanel />
          </div>
        </div>
      );
    }

    return (
      <div className="operations-room operations-room--empty">
        <div className="empty-hint-large">
          <div className="empty-hint-title">Preparing mission</div>
          <div className="empty-hint-body">
            Waiting for workers to report status
            <br />
            and join the operations room.
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="operations-room">
      <ReactFlow
        onInit={(instance) => {
          flowRef.current = instance;
        }}
        nodes={nodes}
        edges={edgeList}
        nodeTypes={NODE_TYPES}
        fitView
        fitViewOptions={FIT_VIEW_OPTIONS}
        minZoom={0.25}
        maxZoom={1.4}
        proOptions={{ hideAttribution: true }}
        nodesConnectable={false}
        elementsSelectable={false}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={28}
          size={1}
          color="var(--line-edge-base)"
        />
      </ReactFlow>
    </div>
  );
}
