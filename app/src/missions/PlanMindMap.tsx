import {
  Background,
  BackgroundVariant,
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type CoordinateExtent,
  type Node,
  type NodeProps,
  useReactFlow,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useMemo, type CSSProperties } from "react";
import {
  buildMindMap,
  type EnvelopeStatus,
  type MindMapEdge,
  type MindMapNode,
  type MindMapNodeData,
  type MindMapNodeType,
  type PlanPayload,
  type PlanSpec,
} from "./plan-mind-map";
import { downloadMindMapSvg } from "./plan-mind-map-export";

interface Props {
  spec: PlanSpec;
  plan: PlanPayload;
  /** Pixel height for the mind-map canvas. Defaults to 280 (matches
   *  the MissionPlanPreview slot). Drawer Plan tab renders taller. */
  height?: number;
}

type RenderNodeData = MindMapNodeData & {
  kind: MindMapNodeType;
};
type PlanMapNode = Node<RenderNodeData, "planMap">;
type PlanMapEdge = Edge<Record<string, unknown>>;

const NODE_TYPES = {
  planMap: PlanMapNodeView,
};

/**
 * QC-3 — Read-only React-Flow visualisation of the proposed plan.
 *
 * Wrapped in ReactFlowProvider so the component is composable inside
 * MissionPlanPreview, the Drawer's Plan tab, and any future surface
 * without leaking provider scope. Users can pan/zoom the canvas, but
 * nodes and edges stay read-only.
 */
export default function PlanMindMap({ spec, plan, height = 280 }: Props) {
  const { nodes, edges, bounds } = useMemo(
    () => buildMindMap(spec, plan),
    [spec, plan],
  );

  const rfNodes = useMemo<PlanMapNode[]>(
    () =>
      nodes.map((n) => ({
        id: n.id,
        type: "planMap",
        position: n.position,
        data: { ...n.data, kind: n.type } satisfies RenderNodeData,
        style: {
          width: n.dimensions.width,
          height: n.dimensions.height,
        },
        className:
          `plan-mind-map__flow-node plan-mind-map__flow-node--${n.type}` +
          ` plan-mind-map__flow-node--${n.data.branch}`,
        selectable: false,
        draggable: false,
        ariaRole: "group",
      })),
    [nodes],
  );
  const rfEdges = useMemo<PlanMapEdge[]>(
    () =>
      edges.map((edge) => ({
        id: edge.id,
        source: edge.source,
        target: edge.target,
        type: edge.type,
        className: edge.className,
        animated: edge.animated,
        data: edge.data ?? {},
      })),
    [edges],
  );
  const translateExtent = useMemo<CoordinateExtent>(
    () => [
      [-240, -240],
      [Math.max(bounds.width, 1) + 240, Math.max(bounds.height, 1) + 240],
    ],
    [bounds.height, bounds.width],
  );

  return (
    <ReactFlowProvider>
      <div
        className="plan-mind-map"
        data-testid="plan-mind-map"
        style={{ height }}
      >
        <ReactFlow
          nodes={rfNodes}
          edges={rfEdges}
          nodeTypes={NODE_TYPES}
          fitView
          fitViewOptions={{ padding: 0.16, minZoom: 0.12, maxZoom: 1.05 }}
          minZoom={0.12}
          maxZoom={1.45}
          translateExtent={translateExtent}
          nodeExtent={translateExtent}
          nodesConnectable={false}
          elementsSelectable={false}
          nodesDraggable={false}
          panOnDrag
          panOnScroll
          zoomOnScroll
          zoomOnPinch
          zoomOnDoubleClick={false}
          onlyRenderVisibleElements
          proOptions={{ hideAttribution: true }}
        >
          <Background
            variant={BackgroundVariant.Dots}
            gap={20}
            size={1}
            color="var(--line-edge-base)"
          />
          <MindMapViewportControls
            bounds={bounds}
            title={spec.title}
            nodes={nodes}
            edges={edges}
          />
        </ReactFlow>
      </div>
    </ReactFlowProvider>
  );
}

function PlanMapNodeView({ data }: NodeProps<PlanMapNode>) {
  return (
    <div
      className={[
        "plan-map-node",
        `plan-map-node--${data.kind}`,
        `plan-map-node--${data.branch}`,
      ].join(" ")}
      title={data.tooltip}
      data-kind={data.kind}
      data-branch={data.branch}
    >
      <Handle
        className="plan-map-node__handle"
        type="target"
        position={Position.Left}
        isConnectable={false}
      />
      <Handle
        className="plan-map-node__handle"
        type="source"
        position={Position.Right}
        isConnectable={false}
      />
      {renderNodeBody(data)}
    </div>
  );
}

function renderNodeBody(data: RenderNodeData) {
  switch (data.kind) {
    case "root":
      return <RootNodeBody data={data} />;
    case "wave":
      return <WaveNodeBody data={data} />;
    case "task":
      return <TaskNodeBody data={data} />;
    case "tech-root":
      return <TechRootBody data={data} />;
    case "tech-leaf":
      return <TechLeafBody data={data} />;
  }
}

function RootNodeBody({ data }: { data: RenderNodeData }) {
  return (
    <>
      <div className="plan-map-node__eyebrow">Mission</div>
      <div className="plan-map-node__title plan-map-node__title--root">
        {data.label}
      </div>
      {data.objective ? (
        <div className="plan-map-node__objective">{data.objective}</div>
      ) : null}
      <EnvelopeMarker
        status={data.envelope_status ?? "unknown"}
        label={data.envelope_label ?? "No envelope"}
      />
    </>
  );
}

function WaveNodeBody({ data }: { data: RenderNodeData }) {
  return (
    <>
      <div className="plan-map-node__eyebrow">{data.subtitle}</div>
      <div className="plan-map-node__title">{data.label}</div>
    </>
  );
}

function TaskNodeBody({ data }: { data: RenderNodeData }) {
  return (
    <>
      <div className="plan-map-node__meta-row">
        <span className="plan-map-node__badge">{roleLabel(data.role)}</span>
        {typeof data.dependency_count === "number" && data.dependency_count > 0 ? (
          <span className="plan-map-node__badge plan-map-node__badge--muted">
            {data.dependency_count} dep
          </span>
        ) : null}
      </div>
      <div className="plan-map-node__title plan-map-node__title--task">
        {data.label}
      </div>
      <div className="plan-map-node__meta-row plan-map-node__meta-row--bottom">
        {data.scope_summary ? (
          <span className="plan-map-node__pill">{data.scope_summary}</span>
        ) : null}
        {data.criteria_summary ? (
          <span className="plan-map-node__pill">criteria</span>
        ) : null}
      </div>
    </>
  );
}

function TechRootBody({ data }: { data: RenderNodeData }) {
  return (
    <>
      <div className="plan-map-node__eyebrow">Stack</div>
      <div className="plan-map-node__title">{data.label}</div>
      <div className="plan-map-node__subtle">{data.subtitle}</div>
    </>
  );
}

function TechLeafBody({ data }: { data: RenderNodeData }) {
  return (
    <>
      <div className="plan-map-node__eyebrow">{data.layer}</div>
      <div className="plan-map-node__title plan-map-node__title--tech">
        {data.choice ?? data.label}
      </div>
      {data.is_new ? <div className="plan-map-node__new">new</div> : null}
    </>
  );
}

function EnvelopeMarker({
  status,
  label,
}: {
  status: EnvelopeStatus;
  label: string;
}) {
  return (
    <div
      className={`plan-map-node__envelope plan-map-node__envelope--${status}`}
    >
      <span className="plan-map-node__envelope-dot" aria-hidden />
      <span>{label}</span>
    </div>
  );
}

function roleLabel(role: unknown): string {
  switch (role) {
    case "tester":
      return "test";
    case "reviewer":
      return "review";
    case "implementer":
      return "build";
    default:
      return "task";
  }
}

function MindMapViewportControls({
  bounds,
  title,
  nodes,
  edges,
}: {
  bounds: { width: number; height: number };
  title: string;
  nodes: MindMapNode[];
  edges: MindMapEdge[];
}) {
  const flow = useReactFlow<PlanMapNode, PlanMapEdge>();
  const fit = () => {
    void flow.fitView({
      padding: 0.16,
      minZoom: 0.12,
      maxZoom: 1.05,
      duration: 180,
    });
  };
  const reset = () => {
    void flow.setViewport({ x: 36, y: 36, zoom: 1 }, { duration: 180 });
  };
  const download = () => {
    void downloadMindMapSvg({ title, nodes, edges, bounds });
  };
  return (
    <div
      className="plan-mind-map__controls"
      style={{
        "--plan-map-width": `${Math.round(bounds.width)}px`,
        "--plan-map-height": `${Math.round(bounds.height)}px`,
      } as CSSProperties}
    >
      <button
        type="button"
        className="plan-mind-map__control"
        onClick={fit}
        aria-label="Fit mind map"
        title="Fit"
      >
        <FitIcon />
      </button>
      <button
        type="button"
        className="plan-mind-map__control"
        onClick={reset}
        aria-label="Reset mind map zoom"
        title="Reset"
      >
        <ResetIcon />
      </button>
      <button
        type="button"
        className="plan-mind-map__control"
        onClick={download}
        aria-label="Download mind map"
        title="Download"
      >
        <DownloadIcon />
      </button>
    </div>
  );
}

function FitIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16" focusable="false">
      <path d="M2 6V2h4M10 2h4v4M14 10v4h-4M6 14H2v-4" />
    </svg>
  );
}

function ResetIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16" focusable="false">
      <path d="M4.2 5.1A5 5 0 1 1 3 8.3M3 3v4h4" />
    </svg>
  );
}

function DownloadIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16" focusable="false">
      <path d="M8 2.5v7M5.2 7.1 8 9.9l2.8-2.8M3 12.7h10" />
    </svg>
  );
}
