import type { LayoutDirection } from "../../generated/api";
import { routeIntersectionDiagnostics, type RouteIntersectionDiagnostics } from "./obstacleRouting";
import type { LayoutElements, RoutePoint, RoutedEdgeSpec } from "./types";

export type GeometryParityReport = {
  edgeId: string;
  channelId?: string;
  turns: number;
  collisions: number;
  trunkDeviation: number;
  routePoints: RoutePoint[];
  intersectionDiagnostics: RouteIntersectionDiagnostics;
};

function turnCount(points: RoutePoint[]): number {
  if (points.length < 3) {
    return 0;
  }
  return points.slice(1, -1).length;
}

function trunkDeviation(edge: RoutedEdgeSpec, layoutDirection: LayoutDirection): number {
  const trunkCoord = edge.trunkCoord;
  if (typeof trunkCoord !== "number") {
    return 0;
  }
  const interior = edge.routePoints.length > 2 ? edge.routePoints.slice(1, -1) : edge.routePoints;
  if (interior.length === 0) {
    return 0;
  }
  const axis = layoutDirection === "Vertical" ? "y" : "x";
  const distances = interior.map((point) => Math.abs(point[axis] - trunkCoord));
  return Math.min(...distances);
}

export function evaluateGeometryParity(
  layout: LayoutElements,
  layoutDirection: LayoutDirection = "Horizontal",
): GeometryParityReport[] {
  return layout.edges.map((edge) => {
    const intersectionDiagnostics = routeIntersectionDiagnostics(edge, layout.nodes);
    return {
      edgeId: edge.id,
      channelId: edge.channelId,
      turns: turnCount(edge.routePoints),
      collisions: intersectionDiagnostics.collisionCount,
      trunkDeviation: trunkDeviation(edge, layoutDirection),
      routePoints: edge.routePoints.map((point) => ({ ...point })),
      intersectionDiagnostics,
    };
  });
}

export function formatGeometryParityFailures(reports: GeometryParityReport[]): string {
  return reports
    .map((report) =>
      JSON.stringify(
        {
          edgeId: report.edgeId,
          channelId: report.channelId ?? null,
          turns: report.turns,
          collisions: report.collisions,
          trunkDeviation: report.trunkDeviation,
          routePoints: report.routePoints,
          intersections: report.intersectionDiagnostics.intersections,
        },
        null,
        2,
      ),
    )
    .join("\n\n");
}
