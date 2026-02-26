import type { LayoutElements, RoutePoint, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

type Rect = {
  id: string;
  left: number;
  right: number;
  top: number;
  bottom: number;
};

const OBSTACLE_PADDING = 18;
const SOURCE_EXIT_X = 40;
const TARGET_ENTRY_X = 40;
const RASTER_STEP = 8;

function snap(value: number): number {
  return Math.round(value / RASTER_STEP) * RASTER_STEP;
}

function nodeRect(node: SemanticNodePlacement, padding = 0): Rect {
  return {
    id: node.id,
    left: node.x - padding,
    right: node.x + node.width + padding,
    top: node.y - padding,
    bottom: node.y + node.height + padding,
  };
}

function hash32(value: string): number {
  let hash = 2166136261;
  for (let idx = 0; idx < value.length; idx += 1) {
    hash ^= value.charCodeAt(idx);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function memberHandleYOffset(node: SemanticNodePlacement, handleId: string): number {
  if (!handleId.includes("member-")) {
    return node.height / 2;
  }
  const slots = Math.max(2, Math.min(14, Math.floor((node.height - 26) / 18)));
  const slot = hash32(`${node.id}:${handleId}`) % slots;
  return 18 + slot * 18;
}

function anchorPoint(
  node: SemanticNodePlacement,
  handleId: string,
  role: "source" | "target",
): RoutePoint {
  const lower = handleId.toLowerCase();
  if (lower.includes("top")) {
    return { x: snap(node.x + node.width / 2), y: snap(node.y) };
  }
  if (lower.includes("bottom")) {
    return { x: snap(node.x + node.width / 2), y: snap(node.y + node.height) };
  }
  if (lower.includes("left") || lower.startsWith("target-")) {
    return { x: snap(node.x), y: snap(node.y + memberHandleYOffset(node, handleId)) };
  }
  if (lower.includes("right") || lower.startsWith("source-")) {
    return { x: snap(node.x + node.width), y: snap(node.y + memberHandleYOffset(node, handleId)) };
  }

  if (role === "target") {
    return { x: snap(node.x), y: snap(node.y + node.height / 2) };
  }
  return { x: snap(node.x + node.width), y: snap(node.y + node.height / 2) };
}

function segmentIntersectsRect(a: RoutePoint, b: RoutePoint, rect: Rect): boolean {
  const minX = Math.min(a.x, b.x);
  const maxX = Math.max(a.x, b.x);
  const minY = Math.min(a.y, b.y);
  const maxY = Math.max(a.y, b.y);

  if (a.x === b.x) {
    return a.x >= rect.left && a.x <= rect.right && maxY >= rect.top && minY <= rect.bottom;
  }
  if (a.y === b.y) {
    return a.y >= rect.top && a.y <= rect.bottom && maxX >= rect.left && minX <= rect.right;
  }

  // Fallback for diagonal segments (rare): overlap bbox and sample midpoint.
  if (maxX < rect.left || minX > rect.right || maxY < rect.top || minY > rect.bottom) {
    return false;
  }
  const midX = (a.x + b.x) / 2;
  const midY = (a.y + b.y) / 2;
  return midX >= rect.left && midX <= rect.right && midY >= rect.top && midY <= rect.bottom;
}

function simplify(points: RoutePoint[]): RoutePoint[] {
  if (points.length <= 2) {
    return points;
  }

  const deduped: RoutePoint[] = [];
  for (const point of points) {
    const prev = deduped[deduped.length - 1];
    if (prev && prev.x === point.x && prev.y === point.y) {
      continue;
    }
    deduped.push({ x: snap(point.x), y: snap(point.y) });
  }

  const simplified: RoutePoint[] = [];
  for (const point of deduped) {
    const prev = simplified[simplified.length - 1];
    const prevPrev = simplified[simplified.length - 2];
    if (prev && prevPrev) {
      const collinear =
        (prevPrev.x === prev.x && prev.x === point.x) ||
        (prevPrev.y === prev.y && prev.y === point.y);
      if (collinear) {
        simplified[simplified.length - 1] = point;
        continue;
      }
    }
    simplified.push(point);
  }
  return simplified;
}

function pathIntersections(points: RoutePoint[], obstacles: Rect[]): number {
  let collisions = 0;
  for (let idx = 1; idx < points.length; idx += 1) {
    const from = points[idx - 1];
    const to = points[idx];
    if (!from || !to) {
      continue;
    }
    for (const obstacle of obstacles) {
      if (segmentIntersectsRect(from, to, obstacle)) {
        collisions += 1;
      }
    }
  }
  return collisions;
}

function routeLength(points: RoutePoint[]): number {
  let total = 0;
  for (let idx = 1; idx < points.length; idx += 1) {
    const from = points[idx - 1];
    const to = points[idx];
    if (!from || !to) {
      continue;
    }
    total += Math.abs(to.x - from.x) + Math.abs(to.y - from.y);
  }
  return total;
}

function trunkPenalty(points: RoutePoint[], trunkCoord: number | undefined): number {
  if (typeof trunkCoord !== "number") {
    return 0;
  }
  const distances = points.slice(1, -1).map((point) => Math.abs(point.x - trunkCoord));
  if (distances.length === 0) {
    return 0;
  }
  return Math.min(...distances) * 0.08;
}

function flowCandidates(
  source: RoutePoint,
  target: RoutePoint,
  preferredX: number,
): RoutePoint[][] {
  const direction = target.x >= source.x ? 1 : -1;
  const midX = (source.x + target.x) / 2;
  const xCandidates = [...new Set([preferredX, midX, preferredX + 72, preferredX - 72].map(snap))];
  const yMid = snap((source.y + target.y) / 2);
  const yCandidates = [...new Set([yMid, yMid + 96, yMid - 96].map(snap))];
  const sourceExit = snap(source.x + direction * SOURCE_EXIT_X);
  const targetEntry = snap(target.x - direction * TARGET_ENTRY_X);

  const candidates: RoutePoint[][] = [];
  for (const x of xCandidates) {
    candidates.push([source, { x, y: source.y }, { x, y: target.y }, target]);
  }

  for (const corridorY of yCandidates) {
    candidates.push([
      source,
      { x: sourceExit, y: source.y },
      { x: sourceExit, y: corridorY },
      { x: targetEntry, y: corridorY },
      { x: targetEntry, y: target.y },
      target,
    ]);
  }

  return candidates;
}

function hierarchyCandidates(
  source: RoutePoint,
  target: RoutePoint,
  preferredY: number,
): RoutePoint[][] {
  const yCandidates = [
    ...new Set([preferredY, (source.y + target.y) / 2, preferredY + 64, preferredY - 64].map(snap)),
  ];
  return yCandidates.map((y) => [source, { x: source.x, y }, { x: target.x, y }, target]);
}

function selectBestPath(
  edge: RoutedEdgeSpec,
  candidates: RoutePoint[][],
  obstacles: Rect[],
): RoutePoint[] {
  let bestPath = simplify(candidates[0] ?? []);
  let bestScore = Number.POSITIVE_INFINITY;

  for (const candidate of candidates) {
    const path = simplify(candidate);
    if (path.length < 2) {
      continue;
    }
    const collisions = pathIntersections(path, obstacles);
    const turns = Math.max(0, path.length - 2);
    const length = routeLength(path);
    const weightBias = Math.max(1, edge.channelWeight ?? edge.bundleCount ?? 1);
    const score =
      collisions * 100_000 +
      turns * (12 + Math.min(8, weightBias * 0.8)) +
      length * 0.035 +
      trunkPenalty(path, edge.trunkCoord);
    if (score < bestScore) {
      bestScore = score;
      bestPath = path;
    }
  }

  return bestPath;
}

function edgeObstacles(allObstacles: Rect[], edge: RoutedEdgeSpec): Rect[] {
  return allObstacles.filter(
    (obstacle) => obstacle.id !== edge.source && obstacle.id !== edge.target,
  );
}

export function routeEdgesWithObstacles(layout: LayoutElements): LayoutElements {
  const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
  const obstacles = layout.nodes.map((node) => nodeRect(node, OBSTACLE_PADDING));

  const edges = layout.edges.map((edge) => {
    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode) {
      return edge;
    }

    const source = anchorPoint(sourceNode, edge.sourceHandle, "source");
    const target = anchorPoint(targetNode, edge.targetHandle, "target");
    const preferredX = edge.trunkCoord ?? snap((source.x + target.x) / 2);
    const preferredY = snap((source.y + target.y) / 2);
    const candidates =
      edge.family === "hierarchy"
        ? hierarchyCandidates(source, target, preferredY)
        : flowCandidates(source, target, preferredX);
    const bestPath = selectBestPath(edge, candidates, edgeObstacles(obstacles, edge));

    return {
      ...edge,
      routePoints: bestPath,
    };
  });

  return {
    ...layout,
    edges,
  };
}

export function routeIntersectsNonEndpointNode(
  edge: RoutedEdgeSpec,
  nodes: SemanticNodePlacement[],
): boolean {
  if (edge.routePoints.length < 2) {
    return false;
  }
  const blocked = nodes
    .filter((node) => node.id !== edge.source && node.id !== edge.target)
    .map((node) => nodeRect(node, 0));
  return pathIntersections(edge.routePoints, blocked) > 0;
}
