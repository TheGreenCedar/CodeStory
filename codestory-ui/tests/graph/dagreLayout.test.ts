import { describe, expect, it } from "vitest";

import { buildDagreLayout } from "../../src/graph/layout/dagreLayout";
import type { LayoutElements } from "../../src/graph/layout/types";

const SEED: LayoutElements = {
  centerNodeId: "runner",
  nodes: [
    {
      id: "svc",
      kind: "CLASS",
      label: "Service",
      center: false,
      nodeStyle: "card",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["svc"],
      memberCount: 1,
      badgeVisibleMembers: 1,
      badgeTotalMembers: 1,
      members: [],
      xRank: 0,
      yRank: 0,
      x: 0,
      y: 0,
      width: 260,
      height: 140,
      isVirtualBundle: false,
    },
    {
      id: "runner",
      kind: "METHOD",
      label: "Service::run",
      center: true,
      nodeStyle: "pill",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["runner"],
      memberCount: 0,
      members: [],
      xRank: 1,
      yRank: 0,
      x: 0,
      y: 0,
      width: 220,
      height: 34,
      isVirtualBundle: false,
    },
    {
      id: "worker",
      kind: "METHOD",
      label: "Worker::execute",
      center: false,
      nodeStyle: "pill",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["worker"],
      memberCount: 0,
      members: [],
      xRank: 2,
      yRank: 0,
      x: 0,
      y: 0,
      width: 220,
      height: 34,
      isVirtualBundle: false,
    },
    {
      id: "helper",
      kind: "METHOD",
      label: "Helper::assist",
      center: false,
      nodeStyle: "pill",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["helper"],
      memberCount: 0,
      members: [],
      xRank: 2,
      yRank: 1,
      x: 0,
      y: 0,
      width: 220,
      height: 34,
      isVirtualBundle: false,
    },
    {
      id: "base",
      kind: "CLASS",
      label: "BaseClass",
      center: false,
      nodeStyle: "card",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["base"],
      memberCount: 0,
      members: [],
      xRank: 2,
      yRank: 2,
      x: 0,
      y: 0,
      width: 250,
      height: 130,
      isVirtualBundle: false,
    },
    {
      id: "child",
      kind: "CLASS",
      label: "ChildClass",
      center: false,
      nodeStyle: "card",
      isNonIndexed: false,
      duplicateCount: 1,
      mergedSymbolIds: ["child"],
      memberCount: 0,
      members: [],
      xRank: 3,
      yRank: 0,
      x: 0,
      y: 0,
      width: 250,
      height: 130,
      isVirtualBundle: false,
    },
  ],
  edges: [
    {
      id: "call-1",
      sourceEdgeIds: ["call-1"],
      source: "runner",
      target: "worker",
      sourceHandle: "source-node",
      targetHandle: "target-node",
      kind: "CALL",
      certainty: null,
      multiplicity: 1,
      family: "flow",
      routeKind: "direct",
      routePoints: [],
    },
    {
      id: "call-2",
      sourceEdgeIds: ["call-2"],
      source: "runner",
      target: "helper",
      sourceHandle: "source-node",
      targetHandle: "target-node",
      kind: "CALL",
      certainty: null,
      multiplicity: 1,
      family: "flow",
      routeKind: "direct",
      routePoints: [],
    },
    {
      id: "inherit-1",
      sourceEdgeIds: ["inherit-1"],
      source: "child",
      target: "base",
      sourceHandle: "source-node",
      targetHandle: "target-node",
      kind: "INHERITANCE",
      certainty: null,
      multiplicity: 1,
      family: "hierarchy",
      routeKind: "hierarchy",
      routePoints: [],
    },
  ],
};

describe("buildDagreLayout", () => {
  it("produces deterministic node placement", () => {
    const first = buildDagreLayout(SEED, "Horizontal");
    const second = buildDagreLayout(SEED, "Horizontal");

    const firstPositions = first.nodes.map((node) => ({ id: node.id, x: node.x, y: node.y }));
    const secondPositions = second.nodes.map((node) => ({ id: node.id, x: node.x, y: node.y }));

    expect(secondPositions).toEqual(firstPositions);
  });

  it("changes dominant axis between horizontal and vertical layouts", () => {
    const horizontal = buildDagreLayout(SEED, "Horizontal");
    const vertical = buildDagreLayout(SEED, "Vertical");

    const horizontalNodes = new Map(horizontal.nodes.map((node) => [node.id, node]));
    const verticalNodes = new Map(vertical.nodes.map((node) => [node.id, node]));

    const horizontalSource = horizontalNodes.get("runner");
    const horizontalTarget = horizontalNodes.get("worker");
    const verticalSource = verticalNodes.get("runner");
    const verticalTarget = verticalNodes.get("worker");

    expect(horizontalSource).toBeDefined();
    expect(horizontalTarget).toBeDefined();
    expect(verticalSource).toBeDefined();
    expect(verticalTarget).toBeDefined();

    if (!horizontalSource || !horizontalTarget || !verticalSource || !verticalTarget) {
      return;
    }

    const horizontalDx = Math.abs(horizontalTarget.x - horizontalSource.x);
    const horizontalDy = Math.abs(horizontalTarget.y - horizontalSource.y);
    const verticalDx = Math.abs(verticalTarget.x - verticalSource.x);
    const verticalDy = Math.abs(verticalTarget.y - verticalSource.y);

    expect(horizontalDx).toBeGreaterThan(horizontalDy);
    expect(verticalDy).toBeGreaterThan(verticalDx);
  });

  it("retains route points for edge midpointing and keyboard navigation", () => {
    const layout = buildDagreLayout(SEED, "Horizontal");

    const routedEdges = layout.edges.filter((edge) => edge.routePoints.length > 0);
    expect(routedEdges.length).toBeGreaterThan(0);
    expect(
      routedEdges.every((edge) =>
        edge.routePoints.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y)),
      ),
    ).toBe(true);
  });

  it("keeps hierarchy edges visually separated", () => {
    const layout = buildDagreLayout(SEED, "Horizontal");

    const byId = new Map(layout.nodes.map((node) => [node.id, node]));
    const hierarchyEdge = layout.edges.find((edge) => edge.kind === "INHERITANCE");
    expect(hierarchyEdge).toBeDefined();
    if (!hierarchyEdge) {
      return;
    }

    const source = byId.get(hierarchyEdge.source);
    const target = byId.get(hierarchyEdge.target);
    expect(source).toBeDefined();
    expect(target).toBeDefined();
    if (!source || !target) {
      return;
    }

    expect(Math.abs(source.x - target.x) + Math.abs(source.y - target.y)).toBeGreaterThan(120);
  });
});
