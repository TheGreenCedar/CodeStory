import { describe, expect, it } from "vitest";

import type { GraphResponse } from "../../src/generated/api";
import { buildDagreLayout } from "../../src/graph/layout/dagreLayout";
import { buildCanonicalLayout } from "../../src/graph/layout/semanticGraph";

const FIXTURE: GraphResponse = {
  center_id: "runner",
  truncated: false,
  nodes: [
    { id: "svc", label: "Service", kind: "CLASS", depth: 0 },
    { id: "runner", label: "Service::run", kind: "METHOD", depth: 0 },
    { id: "worker", label: "Worker::execute", kind: "METHOD", depth: 1 },
    { id: "helper", label: "Helper::assist", kind: "METHOD", depth: 1 },
    { id: "base", label: "BaseClass", kind: "CLASS", depth: 1 },
    { id: "child", label: "ChildClass", kind: "CLASS", depth: 2 },
  ],
  edges: [
    { id: "member-1", source: "svc", target: "runner", kind: "MEMBER" },
    { id: "call-1", source: "runner", target: "worker", kind: "CALL" },
    { id: "call-2", source: "runner", target: "helper", kind: "CALL" },
    { id: "inherit-1", source: "child", target: "base", kind: "INHERITANCE" },
  ],
};

describe("buildDagreLayout", () => {
  it("produces deterministic node placement", () => {
    const seed = buildCanonicalLayout(FIXTURE);
    const first = buildDagreLayout(seed, "Horizontal");
    const second = buildDagreLayout(seed, "Horizontal");

    const firstPositions = first.nodes.map((node) => ({ id: node.id, x: node.x, y: node.y }));
    const secondPositions = second.nodes.map((node) => ({ id: node.id, x: node.x, y: node.y }));

    expect(secondPositions).toEqual(firstPositions);
  });

  it("changes dominant axis between horizontal and vertical layouts", () => {
    const seed = buildCanonicalLayout(FIXTURE);
    const horizontal = buildDagreLayout(seed, "Horizontal");
    const vertical = buildDagreLayout(seed, "Vertical");

    const horizontalXSpread =
      Math.max(...horizontal.nodes.map((node) => node.x)) -
      Math.min(...horizontal.nodes.map((node) => node.x));
    const horizontalYSpread =
      Math.max(...horizontal.nodes.map((node) => node.y)) -
      Math.min(...horizontal.nodes.map((node) => node.y));
    const verticalXSpread =
      Math.max(...vertical.nodes.map((node) => node.x)) -
      Math.min(...vertical.nodes.map((node) => node.x));
    const verticalYSpread =
      Math.max(...vertical.nodes.map((node) => node.y)) -
      Math.min(...vertical.nodes.map((node) => node.y));

    expect(horizontalXSpread).toBeGreaterThan(horizontalYSpread);
    expect(verticalYSpread).toBeGreaterThan(verticalXSpread);
  });

  it("retains route points for edge midpointing and keyboard navigation", () => {
    const seed = buildCanonicalLayout(FIXTURE);
    const layout = buildDagreLayout(seed, "Horizontal");

    const routedEdges = layout.edges.filter((edge) => edge.routePoints.length > 0);
    expect(routedEdges.length).toBeGreaterThan(0);
    expect(
      routedEdges.every((edge) =>
        edge.routePoints.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y)),
      ),
    ).toBe(true);
  });

  it("keeps hierarchy edges visually separated", () => {
    const seed = buildCanonicalLayout(FIXTURE);
    const layout = buildDagreLayout(seed, "Horizontal");

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
