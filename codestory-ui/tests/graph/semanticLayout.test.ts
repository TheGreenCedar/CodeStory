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
          id: "flush_errors",
          label: "WorkspaceIndexer::flush_errors",
          kind: "METHOD",
          depth: 1,
        },
        {
          id: "seed_symbol_table",
          label: "WorkspaceIndexer::seed_symbol_table",
          kind: "METHOD",
          depth: 1,
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
        { id: "member-2", source: "workspace", target: "flush_errors", kind: "MEMBER" },
        { id: "member-3", source: "workspace", target: "seed_symbol_table", kind: "MEMBER" },
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
});
