import { render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { api } from "../../src/api/client";
import { GraphTrailControls } from "../../src/components/GraphTrailControls";
import { defaultTrailUiConfig } from "../../src/graph/trailConfig";

describe("GraphTrailControls", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("does not render a bundling selector", () => {
    render(
      <GraphTrailControls
        config={defaultTrailUiConfig()}
        projectOpen
        projectRevision={0}
        hasRootSymbol
        rootSymbolLabel="WorkspaceIndexer::run_incremental"
        disabledReason={null}
        isRunning={false}
        dialogOpen={false}
        onDialogOpenChange={vi.fn()}
        onConfigChange={vi.fn()}
        onRunTrail={vi.fn()}
        onResetDefaults={vi.fn()}
      />,
    );

    expect(screen.queryByText("Bundling")).not.toBeInTheDocument();
  });

  it("renders node grouping controls", () => {
    render(
      <GraphTrailControls
        config={defaultTrailUiConfig()}
        projectOpen
        projectRevision={0}
        hasRootSymbol
        rootSymbolLabel="WorkspaceIndexer::run_incremental"
        disabledReason={null}
        isRunning={false}
        dialogOpen
        onDialogOpenChange={vi.fn()}
        onConfigChange={vi.fn()}
        onRunTrail={vi.fn()}
        onResetDefaults={vi.fn()}
      />,
    );

    const grouping = screen.getByRole("combobox", { name: "Grouping" });
    expect(within(grouping).getByRole("option", { name: "No Group" })).toBeInTheDocument();
    expect(within(grouping).getByRole("option", { name: "Namespace" })).toBeInTheDocument();
    expect(within(grouping).getByRole("option", { name: "File" })).toBeInTheDocument();
  });

  it("renders custom trail dialog controls when opened", () => {
    render(
      <GraphTrailControls
        config={defaultTrailUiConfig()}
        projectOpen
        projectRevision={0}
        hasRootSymbol
        rootSymbolLabel="WorkspaceIndexer::run_incremental"
        disabledReason={null}
        isRunning={false}
        dialogOpen
        onDialogOpenChange={vi.fn()}
        onConfigChange={vi.fn()}
        onRunTrail={vi.fn()}
        onResetDefaults={vi.fn()}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Custom trail" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Horizontal" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Vertical" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Check All" })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "Uncheck All" })).toHaveLength(2);
  });

  it("refetches trail filter options when project revision changes", async () => {
    const optionsSpy = vi
      .spyOn(api, "graphTrailFilterOptions")
      .mockResolvedValueOnce({
        node_kinds: ["CLASS"],
        edge_kinds: ["CALL"],
      })
      .mockResolvedValueOnce({
        node_kinds: ["METHOD"],
        edge_kinds: ["MEMBER"],
      });

    const props = {
      config: defaultTrailUiConfig(),
      projectOpen: true,
      hasRootSymbol: true,
      rootSymbolLabel: "WorkspaceIndexer::run_incremental",
      disabledReason: null,
      isRunning: false,
      dialogOpen: true,
      onDialogOpenChange: vi.fn(),
      onConfigChange: vi.fn(),
      onRunTrail: vi.fn(),
      onResetDefaults: vi.fn(),
    } as const;

    const { rerender } = render(<GraphTrailControls {...props} projectRevision={1} />);

    await waitFor(() => expect(optionsSpy).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("checkbox", { name: "Class" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Call" })).toBeInTheDocument();

    rerender(<GraphTrailControls {...props} projectRevision={2} />);

    await waitFor(() => expect(optionsSpy).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("checkbox", { name: "Class" })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "Call" })).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Method" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Member" })).toBeInTheDocument();
  });
});
