import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { LayoutDirection } from "../../src/generated/api";
import { runDeterministicParityPipeline } from "../../src/graph/layout/parityPipeline";
import { toReactFlowElements } from "../../src/graph/layout/routing";
import type {
  LayoutElements,
  RoutedEdgeSpec,
  SemanticNodePlacement,
} from "../../src/graph/layout/types";
import { buildEdgePath } from "../../src/graph/render/edgePath";

export type ParitySceneVariant = "horizontal" | "vertical";

type SceneEdge = {
  id: string;
  d: string;
  labelX: number;
  labelY: number;
  stroke: string;
  strokeWidth: number;
  strokeDasharray: string;
  opacity: number;
  interactionWidth: number;
  routeKind: string;
  bundleCount: number;
  routePoints: Array<{ x: number; y: number }>;
};

export type ParityTrailScene = {
  html: string;
  bundledEdgeIds: string[];
};

const FIXTURE_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "fixtures");

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function numericStyleValue(value: unknown, fallback: number): number {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return fallback;
}

function duplicatedEdge(
  edge: RoutedEdgeSpec,
  suffix: string,
  sourceHandle: string,
  targetHandle: string,
): RoutedEdgeSpec {
  const id = `${edge.id}-${suffix}`;
  return {
    ...edge,
    id,
    sourceEdgeIds: [id],
    sourceHandle,
    targetHandle,
    routeKind: "direct",
    bundleCount: 1,
    routePoints: [],
    trunkCoord: undefined,
    channelId: undefined,
    channelPairId: undefined,
    channelWeight: undefined,
    sharedTrunkPoints: undefined,
    sourceMemberOrder: undefined,
    targetMemberOrder: undefined,
  };
}

function readFixture(fileName: string): LayoutElements {
  const fixturePath = path.resolve(FIXTURE_DIR, fileName);
  return JSON.parse(readFileSync(fixturePath, "utf8")) as LayoutElements;
}

function buildHorizontalFixture(): LayoutElements {
  const fixture = readFixture("dense-horizontal.json");
  const edges = fixture.edges.flatMap((edge) => [
    duplicatedEdge(edge, "main", "source-node", "target-node"),
    duplicatedEdge(edge, "alt", "source-node-top", "target-node-bottom"),
  ]);

  return {
    ...fixture,
    nodes: fixture.nodes.map((node) => ({ ...node })),
    edges,
  };
}

function buildVerticalFixture(): LayoutElements {
  const fixture = readFixture("vertical-channel.json");
  const edges = fixture.edges.flatMap((edge) => [
    duplicatedEdge(edge, "main", "source-node", "target-node"),
    duplicatedEdge(edge, "right-left", "source-node-top", "target-node-bottom"),
    duplicatedEdge(edge, "outer", "source-node-right", "target-node-left"),
  ]);

  return {
    ...fixture,
    nodes: fixture.nodes.map((node) => ({ ...node })),
    edges,
  };
}

function buildSceneEdges(routedLayout: LayoutElements, direction: LayoutDirection): SceneEdge[] {
  const flowLayout = toReactFlowElements(routedLayout, direction);

  return flowLayout.edges
    .map((edge) => {
      const routePoints = edge.data?.routePoints ?? [];
      const first = routePoints[0];
      const last = routePoints[routePoints.length - 1];
      if (!first || !last) {
        return null;
      }

      const { path, labelX, labelY } = buildEdgePath(first.x, first.y, last.x, last.y, edge.data);
      return {
        id: edge.id,
        d: path,
        labelX,
        labelY,
        stroke: String(edge.style?.stroke ?? "#8b8f96"),
        strokeWidth: numericStyleValue(edge.style?.strokeWidth, 2.6),
        strokeDasharray:
          typeof edge.style?.strokeDasharray === "string" ? edge.style.strokeDasharray : "none",
        opacity: numericStyleValue(edge.style?.opacity, 1),
        interactionWidth: edge.interactionWidth ?? 20,
        routeKind: edge.data?.routeKind ?? "direct",
        bundleCount: edge.data?.bundleCount ?? 1,
        routePoints,
      };
    })
    .filter((edge): edge is SceneEdge => edge !== null)
    .sort((left, right) => left.id.localeCompare(right.id));
}

function sceneBounds(
  nodes: SemanticNodePlacement[],
  edges: SceneEdge[],
): {
  minX: number;
  minY: number;
  width: number;
  height: number;
} {
  const points = edges.flatMap((edge) => edge.routePoints);
  const xValues = [
    ...nodes.flatMap((node) => [node.x, node.x + node.width]),
    ...points.map((point) => point.x),
  ];
  const yValues = [
    ...nodes.flatMap((node) => [node.y, node.y + node.height]),
    ...points.map((point) => point.y),
  ];
  const minX = Math.min(...xValues) - 96;
  const maxX = Math.max(...xValues) + 96;
  const minY = Math.min(...yValues) - 96;
  const maxY = Math.max(...yValues) + 96;
  return {
    minX,
    minY,
    width: maxX - minX,
    height: maxY - minY,
  };
}

function nodeMarkup(nodes: SemanticNodePlacement[]): string {
  return nodes
    .map((node) => {
      const nodeClass = node.center ? "node center" : "node";
      return `
      <g class="${nodeClass}">
        <rect x="${node.x}" y="${node.y}" width="${node.width}" height="${node.height}" rx="10" />
        <text x="${node.x + node.width / 2}" y="${node.y + node.height / 2}" dominant-baseline="middle" text-anchor="middle">${escapeHtml(node.label)}</text>
      </g>
    `;
    })
    .join("\n");
}

function edgeMarkup(edges: SceneEdge[]): string {
  return edges
    .map((edge) => {
      const style = [
        `--edge-stroke:${edge.stroke}`,
        `--edge-width:${edge.strokeWidth.toFixed(3)}px`,
        `--edge-opacity:${edge.opacity.toFixed(3)}`,
        `--edge-hitbox:${Math.max(20, edge.interactionWidth).toFixed(3)}px`,
        `--edge-dash:${edge.strokeDasharray}`,
      ].join(";");
      return `
      <g class="edge-group" data-edge-id="${escapeHtml(edge.id)}" data-route-kind="${edge.routeKind}" data-bundle-count="${edge.bundleCount}" style="${style}">
        <path class="edge-hitbox" d="${edge.d}" />
        <path class="edge-main" d="${edge.d}" />
        <text class="edge-label" x="${edge.labelX}" y="${edge.labelY}">${escapeHtml(edge.id)}</text>
      </g>
    `;
    })
    .join("\n");
}

function buildVariantScene(variant: ParitySceneVariant): {
  nodes: SemanticNodePlacement[];
  edges: SceneEdge[];
} {
  const direction = variant === "vertical" ? "Vertical" : "Horizontal";
  const fixture = variant === "vertical" ? buildVerticalFixture() : buildHorizontalFixture();
  const parityPipeline = runDeterministicParityPipeline({
    layout: fixture,
    depth: 5,
    nodeCount: fixture.nodes.length,
    edgeCount: fixture.edges.length,
    layoutDirection: direction,
  });
  return {
    nodes: parityPipeline.routed.nodes.map((node) => ({ ...node })),
    edges: buildSceneEdges(parityPipeline.routed, direction),
  };
}

export function buildParityTrailScene(variant: ParitySceneVariant): ParityTrailScene {
  const scene = buildVariantScene(variant);
  const bundledEdgeIds = scene.edges
    .filter((edge) => edge.routeKind === "flow-trunk")
    .map((edge) => edge.id)
    .sort((left, right) => left.localeCompare(right));

  const bounds = sceneBounds(scene.nodes, scene.edges);
  return {
    bundledEdgeIds,
    html: `
    <html>
      <head>
        <meta charset="utf-8" />
        <style>
          :root {
            color-scheme: light;
          }
          body {
            margin: 0;
            background: linear-gradient(180deg, #eef3f7 0%, #dde5ee 100%);
            font-family: "Segoe UI", "Helvetica Neue", sans-serif;
          }
          .workspace-shell {
            width: 1240px;
            height: 760px;
            margin: 24px auto;
            border-radius: 16px;
            box-shadow: 0 18px 48px rgba(34, 50, 70, 0.22);
            overflow: hidden;
            background: radial-gradient(circle at 16% 12%, #f5f8fb 0%, #e8eef5 45%, #dde6ef 100%);
            border: 1px solid rgba(76, 96, 118, 0.18);
          }
          svg {
            width: 100%;
            height: 100%;
            display: block;
          }
          .node {
            pointer-events: none;
          }
          .node rect {
            fill: #f7f9fc;
            stroke: #6b7f95;
            stroke-width: 1.5px;
          }
          .node.center rect {
            stroke-width: 1.8px;
            fill: #f9fbff;
          }
          .node text {
            fill: #283545;
            font-size: 13px;
            letter-spacing: 0.02em;
          }
          .edge-main {
            fill: none;
            stroke: var(--edge-stroke);
            stroke-width: var(--edge-width);
            stroke-dasharray: var(--edge-dash);
            opacity: var(--edge-opacity);
            stroke-linecap: round;
            stroke-linejoin: round;
            transition: stroke-width 100ms ease, opacity 100ms ease, filter 100ms ease;
          }
          .edge-hitbox {
            fill: none;
            stroke: transparent;
            stroke-width: var(--edge-hitbox);
            pointer-events: stroke;
          }
          .edge-label {
            fill: #364c64;
            font-size: 10px;
            letter-spacing: 0.03em;
          }
          .edge-group.hovered .edge-main {
            stroke-width: calc(var(--edge-width) + 0.5px);
            filter: brightness(1.1);
          }
          .edge-group.selected .edge-main {
            stroke-width: calc(var(--edge-width) + 0.55px);
            opacity: 1;
            filter: brightness(0.8) saturate(1.1);
          }
          .scene-badge {
            font-size: 12px;
            fill: #51667c;
            letter-spacing: 0.06em;
            text-transform: uppercase;
          }
        </style>
      </head>
      <body>
        <div class="workspace-shell" data-testid="graph-workspace">
          <svg viewBox="${bounds.minX} ${bounds.minY} ${bounds.width} ${bounds.height}">
            <text class="scene-badge" x="${bounds.minX + 28}" y="${bounds.minY + 30}">
              ${variant} parity scene
            </text>
            <g class="edges">${edgeMarkup(scene.edges)}</g>
            <g class="nodes">${nodeMarkup(scene.nodes)}</g>
          </svg>
        </div>
        <script>
          const workspace = document.querySelector('[data-testid="graph-workspace"]');
          const edgeGroups = [...document.querySelectorAll('.edge-group')];
          const clearClass = (className) => edgeGroups.forEach((entry) => entry.classList.remove(className));

          edgeGroups.forEach((group) => {
            group.addEventListener('mouseenter', () => {
              clearClass('hovered');
              group.classList.add('hovered');
              if (workspace) {
                workspace.dataset.hoveredEdgeId = group.dataset.edgeId ?? '';
              }
            });
            group.addEventListener('mouseleave', () => {
              group.classList.remove('hovered');
              if (workspace?.dataset.hoveredEdgeId === group.dataset.edgeId) {
                delete workspace.dataset.hoveredEdgeId;
              }
            });
            group.addEventListener('click', () => {
              clearClass('selected');
              group.classList.add('selected');
              if (workspace) {
                workspace.dataset.selectedEdgeId = group.dataset.edgeId ?? '';
              }
            });
          });
        </script>
      </body>
    </html>
  `,
  };
}
