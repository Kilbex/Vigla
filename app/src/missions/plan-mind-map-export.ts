import type {
  MindMapBranch,
  MindMapEdge,
  MindMapNode,
  MindMapNodeType,
} from "./plan-mind-map";

interface MindMapExportInput {
  title: string;
  nodes: MindMapNode[];
  edges: MindMapEdge[];
  bounds: { width: number; height: number };
}

interface BranchStyle {
  accent: string;
  border: string;
  fill: string;
}

const EXPORT_PADDING = 56;
const NODE_RADIUS = 8;
const BRANCH_STYLES: Record<MindMapBranch, BranchStyle> = {
  root: {
    accent: "#3fffd0",
    border: "rgba(63, 255, 208, 0.62)",
    fill: "#111827",
  },
  execution: {
    accent: "#3fffd0",
    border: "rgba(63, 255, 208, 0.62)",
    fill: "#111827",
  },
  test: {
    accent: "#8bdc78",
    border: "rgba(139, 220, 120, 0.62)",
    fill: "#101c17",
  },
  review: {
    accent: "#f5c45a",
    border: "rgba(245, 196, 90, 0.66)",
    fill: "#1d1a11",
  },
  tech: {
    accent: "#7ab8ff",
    border: "rgba(122, 184, 255, 0.62)",
    fill: "#101927",
  },
  risk: {
    accent: "#ff5c5c",
    border: "rgba(255, 92, 92, 0.7)",
    fill: "#211216",
  },
};

const TYPE_EXPORT_SCALES: Record<MindMapNodeType, number> = {
  root: 1.08,
  "tech-root": 1.04,
  "tech-leaf": 1.04,
  wave: 1,
  task: 1.05,
};

export async function downloadMindMapSvg(
  input: MindMapExportInput,
): Promise<void> {
  const svg = renderMindMapSvg(input);
  const filename = `${slugify(input.title || "mind-map")}-mind-map.svg`;
  if (isTauriRuntime()) {
    const saved = await saveMindMapViaTauri(filename, svg);
    if (saved) return;
  }
  downloadSvgBlob(filename, svg);
}

export function downloadSvgBlob(filename: string, svg: string): void {
  const blob = new Blob([svg], { type: "image/svg+xml;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.rel = "noopener";
  document.body.append(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

export function renderMindMapSvg({
  title,
  nodes,
  edges,
  bounds,
}: MindMapExportInput): string {
  const mapWidth = Math.max(bounds.width, maxNodeX(nodes), 1);
  const mapHeight = Math.max(bounds.height, maxNodeY(nodes), 1);
  const width = Math.ceil(mapWidth + EXPORT_PADDING * 2);
  const height = Math.ceil(mapHeight + EXPORT_PADDING * 2);
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const edgeMarkup = edges
    .map((edge) => renderEdge(edge, byId))
    .filter(Boolean)
    .join("\n");
  const nodeMarkup = nodes.map(renderNode).join("\n");

  return [
    `<?xml version="1.0" encoding="UTF-8"?>`,
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width * 2}" height="${
      height * 2
    }" viewBox="0 0 ${width} ${height}" role="img" aria-labelledby="title desc">`,
    `<title id="title">${escapeXml(title || "Mission mind map")}</title>`,
    `<desc id="desc">Downloadable mission mind map with scalable text for clear viewing.</desc>`,
    `<defs>`,
    `<filter id="nodeShadow" x="-18%" y="-24%" width="136%" height="152%" color-interpolation-filters="sRGB">`,
    `<feDropShadow dx="0" dy="10" stdDeviation="10" flood-color="#000000" flood-opacity="0.25"/>`,
    `</filter>`,
    `</defs>`,
    `<rect width="100%" height="100%" fill="#030712"/>`,
    `<g transform="translate(${EXPORT_PADDING} ${EXPORT_PADDING})">`,
    `<g fill="none" stroke-linecap="round" stroke-linejoin="round">${edgeMarkup}</g>`,
    `<g>${nodeMarkup}</g>`,
    `</g>`,
    `</svg>`,
  ].join("\n");
}

function renderEdge(
  edge: MindMapEdge,
  byId: Map<string, MindMapNode>,
): string | null {
  const source = byId.get(edge.source);
  const target = byId.get(edge.target);
  if (!source || !target) return null;

  const sourceX = source.position.x + source.dimensions.width;
  const sourceY = source.position.y + source.dimensions.height / 2;
  const targetX = target.position.x;
  const targetY = target.position.y + target.dimensions.height / 2;
  const distance = Math.abs(targetX - sourceX);
  const bend = Math.max(42, distance * 0.42);
  const sourceControlX = sourceX + bend;
  const targetControlX = targetX - bend;
  const branch = edge.data?.branch ?? "execution";
  const style = BRANCH_STYLES[branch];
  const opacity = edge.data?.kind === "dependency" ? 0.38 : 0.72;
  const width = edge.data?.kind === "dependency" ? 1.4 : 2;
  const dash = edge.data?.kind === "dependency" ? ` stroke-dasharray="6 7"` : "";

  return `<path d="M ${round(sourceX)} ${round(sourceY)} C ${round(
    sourceControlX,
  )} ${round(sourceY)}, ${round(targetControlX)} ${round(targetY)}, ${round(
    targetX,
  )} ${round(targetY)}" stroke="${style.accent}" stroke-width="${width}" opacity="${opacity}"${dash}/>`;
}

async function saveMindMapViaTauri(
  filename: string,
  svg: string,
): Promise<boolean> {
  try {
    const { commands } = await import("../bindings");
    const result = await commands.saveMindMapFile(filename, svg);
    if (result.status === "error") throw new Error(result.error);
    return true;
  } catch (error) {
    console.warn(
      "Mind map SVG save failed; falling back to browser download.",
      error,
    );
    return false;
  }
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function renderNode(node: MindMapNode): string {
  const scale = TYPE_EXPORT_SCALES[node.type];
  const width = node.dimensions.width * scale;
  const height = node.dimensions.height * scale;
  const x = node.position.x - (width - node.dimensions.width) / 2;
  const y = node.position.y - (height - node.dimensions.height) / 2;
  const style = BRANCH_STYLES[node.data.branch] ?? BRANCH_STYLES.execution;
  const body = renderNodeBody(node, width, height, style);

  return [
    `<g transform="translate(${round(x)} ${round(y)})" filter="url(#nodeShadow)">`,
    `<rect x="0" y="0" width="${round(width)}" height="${round(height)}" rx="${NODE_RADIUS}" fill="${style.fill}" stroke="${style.border}" stroke-width="1.2"/>`,
    `<rect x="0.5" y="0.5" width="3.5" height="${round(height - 1)}" rx="2" fill="${style.accent}" opacity="0.86"/>`,
    body,
    `</g>`,
  ].join("\n");
}

function renderNodeBody(
  node: MindMapNode,
  width: number,
  height: number,
  style: BranchStyle,
): string {
  switch (node.type) {
    case "root":
      return renderRootNode(node, width, height, style);
    case "wave":
      return renderSimpleNode(
        node.data.subtitle ?? "Wave",
        node.data.label,
        width,
        style,
      );
    case "task":
      return renderTaskNode(node, width, height, style);
    case "tech-root":
      return renderSimpleNode("Stack", node.data.label, width, style);
    case "tech-leaf":
      return renderTechLeafNode(node, width, height, style);
  }
}

function renderRootNode(
  node: MindMapNode,
  width: number,
  height: number,
  style: BranchStyle,
): string {
  const titleLines = wrapText(node.data.label, width - 30, 15, 2);
  const objectiveLines = wrapText(node.data.objective ?? "", width - 30, 11, 2);
  const envelope = node.data.envelope_label ?? "No envelope";
  return [
    renderLabel("Mission", 14, 19, style.accent),
    renderMultilineText(titleLines, 14, 38, 15, 18, "#f8fafc", 700),
    objectiveLines.length > 0
      ? renderMultilineText(objectiveLines, 14, 70, 11, 14, "#cbd5e1", 500)
      : "",
    renderPill(envelope, 14, height - 23, style.accent),
  ].join("\n");
}

function renderTaskNode(
  node: MindMapNode,
  width: number,
  height: number,
  style: BranchStyle,
): string {
  const role = roleLabel(node.data.role);
  const titleLines = wrapText(node.data.label, width - 28, 13, 3);
  const depCount = typeof node.data.dependency_count === "number"
    ? node.data.dependency_count
    : 0;
  const bottomPills = [
    node.data.scope_summary,
    node.data.criteria_summary ? "criteria" : undefined,
  ].filter(
    (value): value is string => typeof value === "string" && value.length > 0,
  );

  return [
    renderPill(role, 14, 13, style.accent),
    depCount > 0 ? renderPill(`${depCount} dep`, 72, 13, "#94a3b8") : "",
    renderMultilineText(titleLines, 14, 42, 13, 16, "#f8fafc", 680),
    bottomPills
      .slice(0, 2)
      .map((pill, index) =>
        renderPill(pill, 14 + index * 94, height - 24, "#94a3b8"),
      )
      .join("\n"),
  ].join("\n");
}

function renderTechLeafNode(
  node: MindMapNode,
  width: number,
  height: number,
  style: BranchStyle,
): string {
  const title = String(node.data.choice ?? node.data.label);
  const titleLines = wrapText(title, width - 30, 14, 2);
  return [
    renderLabel(String(node.data.layer ?? "Layer"), 14, 20, style.accent),
    renderMultilineText(titleLines, 14, 43, 14, 17, "#f8fafc", 680),
    node.data.is_new
      ? renderPill("new", width - 54, height - 24, style.accent)
      : "",
  ].join("\n");
}

function renderSimpleNode(
  label: string,
  title: string,
  width: number,
  style: BranchStyle,
): string {
  const titleLines = wrapText(title, width - 30, 13, 2);
  return [
    renderLabel(label, 14, 19, style.accent),
    renderMultilineText(titleLines, 14, 40, 13, 16, "#f8fafc", 680),
  ].join("\n");
}

function renderLabel(
  text: string,
  x: number,
  y: number,
  color: string,
): string {
  return `<text x="${round(x)}" y="${round(y)}" fill="${color}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" font-weight="700" letter-spacing="0.6">${escapeXml(
    text.toUpperCase(),
  )}</text>`;
}

function renderPill(
  text: string,
  x: number,
  y: number,
  color: string,
): string {
  const label = truncateText(text, 18);
  const width = Math.max(42, Math.min(118, label.length * 6.2 + 18));
  return [
    `<g transform="translate(${round(x)} ${round(y)})">`,
    `<rect x="0" y="0" width="${round(width)}" height="18" rx="9" fill="${color}" opacity="0.12" stroke="${color}" stroke-opacity="0.36"/>`,
    `<text x="9" y="12.5" fill="${color}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="9" font-weight="700">${escapeXml(
      label,
    )}</text>`,
    `</g>`,
  ].join("\n");
}

function renderMultilineText(
  lines: string[],
  x: number,
  y: number,
  fontSize: number,
  lineHeight: number,
  fill: string,
  weight: number,
): string {
  if (lines.length === 0) return "";
  const tspans = lines
    .map(
      (line, index) =>
        `<tspan x="${round(x)}" dy="${index === 0 ? 0 : lineHeight}">${escapeXml(
          line,
        )}</tspan>`,
    )
    .join("");
  return `<text x="${round(x)}" y="${round(y)}" fill="${fill}" font-family="Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, sans-serif" font-size="${fontSize}" font-weight="${weight}">${tspans}</text>`;
}

function wrapText(
  value: string,
  maxWidth: number,
  fontSize: number,
  maxLines: number,
): string[] {
  const normalized = value.replace(/\s+/g, " ").trim();
  if (!normalized) return [];
  const maxChars = Math.max(8, Math.floor(maxWidth / (fontSize * 0.55)));
  const words = normalized.split(" ");
  const lines: string[] = [];
  let current = "";

  for (const word of words) {
    const candidates = splitLongWord(word, maxChars);
    for (const part of candidates) {
      const next = current ? `${current} ${part}` : part;
      if (next.length <= maxChars) {
        current = next;
        continue;
      }
      if (current) lines.push(current);
      current = part;
      if (lines.length === maxLines) break;
    }
    if (lines.length === maxLines) break;
  }
  if (current && lines.length < maxLines) lines.push(current);
  if (lines.length === maxLines) {
    lines[maxLines - 1] = truncateText(lines[maxLines - 1], maxChars);
  }
  return lines;
}

function splitLongWord(word: string, maxChars: number): string[] {
  if (word.length <= maxChars) return [word];
  const parts: string[] = [];
  for (let index = 0; index < word.length; index += maxChars) {
    parts.push(word.slice(index, index + maxChars));
  }
  return parts;
}

function truncateText(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  if (maxChars <= 3) return value.slice(0, maxChars);
  return `${value.slice(0, maxChars - 3)}...`;
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

function maxNodeX(nodes: MindMapNode[]): number {
  return nodes.reduce(
    (max, node) => Math.max(max, node.position.x + node.dimensions.width),
    0,
  );
}

function maxNodeY(nodes: MindMapNode[]): number {
  return nodes.reduce(
    (max, node) => Math.max(max, node.position.y + node.dimensions.height),
    0,
  );
}

function slugify(value: string): string {
  const slug = value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 72);
  return slug || "mind-map";
}

function escapeXml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

function round(value: number): number {
  return Math.round(value * 10) / 10;
}
