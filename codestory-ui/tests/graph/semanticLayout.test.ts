import { describe, expect, it } from "vitest";

import type { GraphResponse } from "../../src/generated/api";
import { buildSemanticLayout } from "../../src/graph/layout/semanticLayout";

describe("buildSemanticLayout", () => {
  it("keeps focal member data visible on the center host node", () => {
    const graph: GraphResponse = {
      center_id: "run_incremental",
      truncated: false,
      nodes: [
        {
          id: "workspace",
          label: "WorkspaceIndexer",
          kind: "CLASS",
          depth: 0,
          badge_visible_members: 8,
          badge_total_members: 82,
        },
        {
          id: "run_incremental",
          label: "WorkspaceIndexer::run_incremental",
          kind: "METHOD",
          depth: 0,
        },
        {
          id: "merge",
          label: "IntermediateStorage::merge",
          kind: "METHOD",
          depth: 1,
        },
      ],
      edges: [
        { id: "member-1", source: "workspace", target: "run_incremental", kind: "MEMBER" },
        { id: "call-1", source: "run_incremental", target: "merge", kind: "CALL" },
      ],
    };

    const layout = buildSemanticLayout(graph);
    const center = layout.nodes.find((node) => node.center);

    expect(center).toBeDefined();
    expect(center?.id).toBe("workspace");
    expect(center?.badgeVisibleMembers).toBe(8);
    expect(center?.badgeTotalMembers).toBe(82);
    expect(center?.members.some((member) => member.id === "run_incremental")).toBe(true);
    expect(
      layout.edges.some(
        (edge) => edge.sourceHandle === "source-member-run_incremental" && edge.kind === "CALL",
      ),
    ).toBe(true);
  });

  it("creates host cards for detached qualified member symbols", () => {
    const graph: GraphResponse = {
      center_id: "run",
      truncated: false,
      nodes: [
        {
          id: "tic",
          label: "TicTacToe",
          kind: "CLASS",
          depth: 0,
          badge_visible_members: 2,
          badge_total_members: 2,
        },
        {
          id: "run",
          label: "TicTacToe::run",
          kind: "FUNCTION",
          depth: 0,
        },
        {
          id: "field_is_draw",
          label: "Field::is_draw",
          kind: "FUNCTION",
          depth: 1,
        },
        {
          id: "field_make_move",
          label: "Field::make_move",
          kind: "FUNCTION",
          depth: 1,
        },
      ],
      edges: [
        { id: "member-1", source: "tic", target: "run", kind: "MEMBER" },
        { id: "call-1", source: "run", target: "field_is_draw", kind: "CALL" },
        { id: "call-2", source: "run", target: "field_make_move", kind: "CALL" },
      ],
    };

    const layout = buildSemanticLayout(graph);
    const fieldHost = layout.nodes.find((node) => node.label === "Field");

    expect(fieldHost).toBeDefined();
    expect(fieldHost?.nodeStyle).toBe("card");
    expect(fieldHost?.members.map((member) => member.id).sort()).toEqual([
      "field_is_draw",
      "field_make_move",
    ]);
  });

  it("lays out nodes with non-overlapping coordinates in a small dense graph", () => {
    const graph: GraphResponse = {
      center_id: "center_fn",
      truncated: false,
      nodes: [
        { id: "host", label: "Host", kind: "CLASS", depth: 0, badge_visible_members: 1 },
        { id: "center_fn", label: "Host::run", kind: "METHOD", depth: 0 },
        ...Array.from({ length: 12 }, (_, idx) => ({
          id: `fn-${idx}`,
          label: `Worker::run_${idx}`,
          kind: "METHOD" as const,
          depth: 1,
        })),
      ],
      edges: [
        { id: "member-root", source: "host", target: "center_fn", kind: "MEMBER" as const },
        ...Array.from({ length: 12 }, (_, idx) => ({
          id: `call-${idx}`,
          source: "center_fn",
          target: `fn-${idx}`,
          kind: "CALL" as const,
        })),
      ],
    };

    const layout = buildSemanticLayout(graph);

    for (let i = 0; i < layout.nodes.length; i += 1) {
      const a = layout.nodes[i];
      if (!a) {
        continue;
      }
      for (let j = i + 1; j < layout.nodes.length; j += 1) {
        const b = layout.nodes[j];
        if (!b) {
          continue;
        }
        const overlaps =
          a.x < b.x + b.width &&
          a.x + a.width > b.x &&
          a.y < b.y + b.height &&
          a.y + a.height > b.y;
        expect(overlaps, `${a.id} overlaps ${b.id}`).toBe(false);
      }
    }
  });
});
