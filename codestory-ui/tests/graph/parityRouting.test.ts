import { describe, expect, it } from "vitest";

import { runDeterministicParityPipeline } from "../../src/graph/layout/parityPipeline";
import {
  evaluateGeometryParity,
  formatGeometryParityFailures,
} from "../../src/graph/layout/parityValidation";
import type { LayoutElements } from "../../src/graph/layout/types";
import blockedCorridorRegression from "./fixtures/blocked-corridor-regression.json";
import denseHorizontal from "./fixtures/dense-horizontal.json";
import verticalChannel from "./fixtures/vertical-channel.json";

function runFixture(
  fixture: LayoutElements,
  layoutDirection: "Horizontal" | "Vertical",
): ReturnType<typeof runDeterministicParityPipeline> {
  return runDeterministicParityPipeline({
    layout: fixture,
    depth: 5,
    nodeCount: fixture.nodes.length,
    edgeCount: fixture.edges.length,
    layoutDirection,
  });
}

function routeSnapshot(layout: LayoutElements): Array<{
  id: string;
  routeKind: string;
  channelId?: string;
  trunkCoord?: number;
  sourceMemberOrder?: number;
  targetMemberOrder?: number;
  routePoints: Array<{ x: number; y: number }>;
}> {
  return [...layout.edges]
    .sort((left, right) => left.id.localeCompare(right.id))
    .map((edge) => ({
      id: edge.id,
      routeKind: edge.routeKind,
      channelId: edge.channelId,
      trunkCoord: edge.trunkCoord,
      sourceMemberOrder: edge.sourceMemberOrder,
      targetMemberOrder: edge.targetMemberOrder,
      routePoints: edge.routePoints,
    }));
}

describe("graph parity geometry harness", () => {
  it("validates fixture-driven collisions, turns, and trunk adherence", () => {
    const fixtures = [
      {
        name: "dense-horizontal",
        layout: denseHorizontal as LayoutElements,
        direction: "Horizontal" as const,
        collisionAllowanceByEdge: {
          "call-3": 1,
        } as Record<string, number>,
      },
      {
        name: "vertical-channel",
        layout: verticalChannel as LayoutElements,
        direction: "Vertical" as const,
        collisionAllowanceByEdge: {} as Record<string, number>,
        maxTrunkDeviation: 192,
      },
      {
        name: "blocked-corridor-regression",
        layout: blockedCorridorRegression as LayoutElements,
        direction: "Horizontal" as const,
        collisionAllowanceByEdge: {} as Record<string, number>,
      },
    ];

    const failures: Array<{ fixture: string; message: string }> = [];

    for (const fixture of fixtures) {
      const pipeline = runFixture(fixture.layout, fixture.direction);
      const reports = evaluateGeometryParity(pipeline.routed, fixture.direction);
      const offenders = reports.filter((report) => {
        const allowedCollisions = fixture.collisionAllowanceByEdge[report.edgeId] ?? 0;
        if (report.collisions > allowedCollisions) {
          return true;
        }
        if (
          report.channelId &&
          typeof fixture.maxTrunkDeviation === "number" &&
          report.trunkDeviation > fixture.maxTrunkDeviation
        ) {
          return true;
        }
        return report.turns > 8;
      });
      if (offenders.length > 0) {
        failures.push({
          fixture: fixture.name,
          message: formatGeometryParityFailures(offenders),
        });
      }
    }

    expect(
      failures,
      failures.map((failure) => `${failure.fixture}\n${failure.message}`).join("\n\n"),
    ).toEqual([]);
  });

  it("keeps route points and channel metadata deterministic by snapshot", () => {
    const horizontal = runFixture(denseHorizontal as LayoutElements, "Horizontal");
    const vertical = runFixture(verticalChannel as LayoutElements, "Vertical");

    expect({
      horizontal: routeSnapshot(horizontal.routed),
      vertical: routeSnapshot(vertical.routed),
    }).toMatchSnapshot();
  });

  it("covers known regression scenes for blocked corridors and vertical channels", () => {
    const blocked = runFixture(blockedCorridorRegression as LayoutElements, "Horizontal");
    const blockedEdge = blocked.routed.edges.find((edge) => edge.id === "blocked-main");
    const blockedReport = evaluateGeometryParity(blocked.routed, "Horizontal").find(
      (report) => report.edgeId === "blocked-main",
    );
    const blocker = (blockedCorridorRegression as LayoutElements).nodes.find(
      (node) => node.id === "blocker",
    );
    expect(blockedEdge).toBeDefined();
    expect(blockedReport).toBeDefined();
    expect(blocker).toBeDefined();
    if (!blockedEdge || !blockedReport || !blocker) {
      throw new Error("missing blocked corridor regression fixtures");
    }
    expect(blockedReport.collisions).toBe(0);

    const blockerLeft = blocker.x;
    const blockerRight = blocker.x + blocker.width;
    const blockerTop = blocker.y;
    const blockerBottom = blocker.y + blocker.height;
    const hasBypassSegment = blockedEdge.routePoints.some((point, index, points) => {
      if (index === 0) {
        return false;
      }
      const previous = points[index - 1];
      if (!previous || previous.y !== point.y) {
        return false;
      }
      const minX = Math.min(previous.x, point.x);
      const maxX = Math.max(previous.x, point.x);
      return minX < blockerLeft && maxX > blockerRight && point.y > blockerBottom;
    });
    expect(hasBypassSegment).toBe(true);

    const crossesBlockedCorridor = blockedEdge.routePoints.some((point, index, points) => {
      if (index === 0) {
        return false;
      }
      const previous = points[index - 1];
      if (!previous || previous.x !== point.x) {
        return false;
      }
      if (point.x < blockerLeft || point.x > blockerRight) {
        return false;
      }
      const minY = Math.min(previous.y, point.y);
      const maxY = Math.max(previous.y, point.y);
      return maxY >= blockerTop && minY <= blockerBottom;
    });
    expect(crossesBlockedCorridor).toBe(false);

    const vertical = runFixture(verticalChannel as LayoutElements, "Vertical");
    const verticalTrunks = vertical.routed.edges.filter((edge) => edge.routeKind === "flow-trunk");
    const verticalReports = evaluateGeometryParity(vertical.routed, "Vertical").filter(
      (report) => report.channelId,
    );
    expect(verticalTrunks.length).toBeGreaterThan(0);
    expect(verticalReports.length).toBe(verticalTrunks.length);
    expect(verticalReports.every((report) => report.collisions === 0)).toBe(true);
    expect(
      verticalReports.every(
        (report) => report.trunkDeviation >= 160 && report.trunkDeviation <= 192,
      ),
    ).toBe(true);
  });
});
