import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { GraphTrailControls } from "../../src/components/GraphTrailControls";
import { defaultTrailUiConfig } from "../../src/graph/trailConfig";

describe("GraphTrailControls", () => {
  it("does not render a bundling selector", () => {
    render(
      <GraphTrailControls
        config={defaultTrailUiConfig()}
        projectOpen
        hasRootSymbol
        disabledReason={null}
        isRunning={false}
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
        hasRootSymbol
        disabledReason={null}
        isRunning={false}
        onConfigChange={vi.fn()}
        onRunTrail={vi.fn()}
        onResetDefaults={vi.fn()}
      />,
    );

    const grouping = screen.getByRole("group", { name: "Node grouping" });
    expect(within(grouping).getByRole("button", { name: "No Group" })).toBeInTheDocument();
    expect(within(grouping).getByRole("button", { name: "Namespace" })).toBeInTheDocument();
    expect(within(grouping).getByRole("button", { name: "File" })).toBeInTheDocument();
  });
});
