import { PARITY_CONSTANTS } from "../layout/parityConstants";
import type { SemanticEdgeData } from "../layout/routing";

type Point = { x: number; y: number };

function orthogonalPath(points: Point[]): string {
  if (points.length === 0) {
    return "";
  }
  const [first, ...rest] = points;
  if (!first) {
    return "";
  }

  let path = `M ${first.x} ${first.y}`;
  for (const point of rest) {
    path += ` L ${point.x} ${point.y}`;
  }
  return path;
}

function clampedElbowCoord(
  sourceCoord: number,
  targetCoord: number,
  desiredCoord: number,
  gutter: number = PARITY_CONSTANTS.rendering.directElbowGutter,
): number {
  if (targetCoord >= sourceCoord) {
    const min = sourceCoord + gutter;
    const max = Math.max(min, targetCoord - gutter);
    return Math.min(max, Math.max(min, desiredCoord));
  }

  const max = sourceCoord - gutter;
  const min = Math.min(max, targetCoord + gutter);
  return Math.max(min, Math.min(max, desiredCoord));
}

function makeRoundedOrthogonalPath(points: Point[]): string {
  if (points.length < 3) {
    return orthogonalPath(points);
  }
  const cornerRadius = PARITY_CONSTANTS.rendering.cornerRadius;
  const firstPoint = points[0];
  if (!firstPoint) {
    return "";
  }

  let path = `M ${firstPoint.x} ${firstPoint.y}`;
  for (let i = 1; i < points.length - 1; i += 1) {
    const prev = points[i - 1];
    const curr = points[i];
    const next = points[i + 1];
    if (!prev || !curr || !next) {
      continue;
    }

    const dist1 = Math.sqrt((curr.x - prev.x) ** 2 + (curr.y - prev.y) ** 2);
    const dist2 = Math.sqrt((next.x - curr.x) ** 2 + (next.y - curr.y) ** 2);
    const radius = Math.min(cornerRadius, dist1 / 2, dist2 / 2);

    const dir1X = Math.sign(curr.x - prev.x);
    const dir1Y = Math.sign(curr.y - prev.y);
    const dir2X = Math.sign(next.x - curr.x);
    const dir2Y = Math.sign(next.y - curr.y);

    const arcStartX = curr.x - dir1X * radius;
    const arcStartY = curr.y - dir1Y * radius;
    const arcEndX = curr.x + dir2X * radius;
    const arcEndY = curr.y + dir2Y * radius;

    const crossProduct = dir1X * dir2Y - dir1Y * dir2X;
    const sweepFlag = crossProduct > 0 ? 1 : 0;

    path += ` L ${arcStartX} ${arcStartY} A ${radius} ${radius} 0 0 ${sweepFlag} ${arcEndX} ${arcEndY}`;
  }

  const last = points[points.length - 1];
  if (!last) {
    return path;
  }
  path += ` L ${last.x} ${last.y}`;
  return path;
}

function isApproximately(value: number, target: number, tolerance = 2): boolean {
  return Math.abs(value - target) <= tolerance;
}

function snapToRaster(value: number): number {
  return Math.round(value / PARITY_CONSTANTS.rasterStep) * PARITY_CONSTANTS.rasterStep;
}

export function decorateBundledTrunkJoins(
  points: Point[],
  data: SemanticEdgeData | undefined,
): Point[] {
  if (!data || data.routeKind !== "flow-trunk" || data.layoutDirection === "Vertical") {
    return points;
  }
  if (points.length < 4) {
    return points;
  }

  const snappedTrunkCoord =
    typeof data.trunkCoord === "number" && Number.isFinite(data.trunkCoord)
      ? snapToRaster(data.trunkCoord)
      : undefined;
  const trunkTolerance = Math.max(2, Math.floor(PARITY_CONSTANTS.rasterStep / 2));

  const decorated: Point[] = [points[0]!];
  for (let idx = 1; idx < points.length - 1; idx += 1) {
    const point = points[idx];
    const next = points[idx + 1];
    const prev = decorated[decorated.length - 1];
    if (!point || !next || !prev) {
      continue;
    }

    const nearTrunk =
      typeof snappedTrunkCoord === "number"
        ? isApproximately(point.x, snappedTrunkCoord, trunkTolerance)
        : true;
    const onVerticalTrunk = isApproximately(prev.x, point.x) && !isApproximately(prev.y, point.y);
    const turningOut =
      isApproximately(next.y, point.y) && !isApproximately(next.x, point.x) && nearTrunk;
    if (!(nearTrunk && onVerticalTrunk && turningOut)) {
      decorated.push(point);
      continue;
    }

    const verticalSegment = Math.abs(point.y - prev.y);
    const horizontalSegment = Math.abs(next.x - point.x);
    const hookRadius = Math.min(
      PARITY_CONSTANTS.rendering.trunkJoinHookRadius,
      Math.floor(verticalSegment / 3),
      Math.floor(horizontalSegment / 4),
    );
    const hookDepth = Math.min(
      PARITY_CONSTANTS.rendering.trunkJoinHookDepth,
      Math.floor(horizontalSegment / 3),
    );
    if (
      hookRadius < PARITY_CONSTANTS.rendering.trunkJoinMinRadius ||
      hookDepth < PARITY_CONSTANTS.rendering.trunkJoinMinDepth
    ) {
      decorated.push(point);
      continue;
    }

    const trunkDirectionY = Math.sign(point.y - prev.y) || 1;
    const targetDirectionX = Math.sign(next.x - point.x);
    if (targetDirectionX === 0) {
      decorated.push(point);
      continue;
    }

    const upperY = point.y - trunkDirectionY * hookRadius;
    const lowerY = point.y + trunkDirectionY * hookRadius;
    const outwardX = point.x + targetDirectionX * hookDepth;
    const clampedOutwardX =
      targetDirectionX > 0
        ? Math.min(next.x - hookRadius, outwardX)
        : Math.max(next.x + hookRadius, outwardX);
    if (
      (targetDirectionX > 0 && clampedOutwardX <= point.x) ||
      (targetDirectionX < 0 && clampedOutwardX >= point.x)
    ) {
      decorated.push(point);
      continue;
    }

    decorated.push(
      { x: point.x, y: upperY },
      { x: clampedOutwardX, y: upperY },
      { x: clampedOutwardX, y: lowerY },
      { x: point.x, y: lowerY },
      point,
    );
  }

  const last = points[points.length - 1];
  if (last) {
    decorated.push(last);
  }
  return decorated;
}

export function buildEdgePath(
  sourceX: number,
  sourceY: number,
  targetX: number,
  targetY: number,
  data: SemanticEdgeData | undefined,
): { path: string; labelX: number; labelY: number } {
  const routedPoints = data?.routePoints ?? [];
  if (routedPoints.length >= 2) {
    const decorativePoints = decorateBundledTrunkJoins(
      routedPoints.map((point) => ({ ...point })),
      data,
    );
    decorativePoints[0] = { x: sourceX, y: sourceY };
    decorativePoints[decorativePoints.length - 1] = { x: targetX, y: targetY };
    const path = makeRoundedOrthogonalPath(decorativePoints);
    const mid = decorativePoints[Math.floor(decorativePoints.length / 2)] ?? {
      x: (sourceX + targetX) / 2,
      y: (sourceY + targetY) / 2,
    };
    return { path, labelX: mid.x, labelY: mid.y };
  }

  const routeKind = data?.routeKind ?? "direct";
  const vertical = data?.layoutDirection === "Vertical";
  if (routeKind === "flow-trunk" || routeKind === "flow-branch") {
    if (vertical) {
      const trunkCoord = data?.trunkCoord ?? (sourceY + targetY) / 2;
      const elbowY = clampedElbowCoord(
        sourceY,
        targetY,
        trunkCoord,
        PARITY_CONSTANTS.rendering.trunkElbowGutter,
      );
      const path = makeRoundedOrthogonalPath([
        { x: sourceX, y: sourceY },
        { x: sourceX, y: elbowY },
        { x: targetX, y: elbowY },
        { x: targetX, y: targetY },
      ]);
      return { path, labelX: (sourceX + targetX) / 2, labelY: elbowY };
    }

    const trunkCoord = data?.trunkCoord ?? (sourceX + targetX) / 2;
    const elbowX = clampedElbowCoord(
      sourceX,
      targetX,
      trunkCoord,
      PARITY_CONSTANTS.rendering.trunkElbowGutter,
    );
    const path = makeRoundedOrthogonalPath([
      { x: sourceX, y: sourceY },
      { x: elbowX, y: sourceY },
      { x: elbowX, y: targetY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: elbowX, labelY: (sourceY + targetY) / 2 };
  }

  if (vertical) {
    const midY = (sourceY + targetY) / 2;
    const path = makeRoundedOrthogonalPath([
      { x: sourceX, y: sourceY },
      { x: sourceX, y: midY },
      { x: targetX, y: midY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: (sourceX + targetX) / 2, labelY: midY };
  }

  const midX = (sourceX + targetX) / 2;
  const path = makeRoundedOrthogonalPath([
    { x: sourceX, y: sourceY },
    { x: midX, y: sourceY },
    { x: midX, y: targetY },
    { x: targetX, y: targetY },
  ]);
  return { path, labelX: midX, labelY: (sourceY + targetY) / 2 };
}
