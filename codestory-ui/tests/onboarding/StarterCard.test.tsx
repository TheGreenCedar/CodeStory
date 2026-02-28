import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { StarterCard } from "../../src/features/onboarding/StarterCard";

describe("StarterCard", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("shows open project action first", () => {
    const onOpenProject = vi.fn();
    render(
      <StarterCard
        projectPath="."
        projectOpen={false}
        indexComplete={false}
        askedFirstQuestion={false}
        inspectedSource={false}
        onOpenProject={onOpenProject}
        onRunIndex={vi.fn()}
        onSeedQuestion={vi.fn()}
        onInspectSource={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Open project/i }));
    expect(onOpenProject).toHaveBeenCalledTimes(1);
  });

  it("shows ready state when all steps completed", () => {
    render(
      <StarterCard
        projectPath="."
        projectOpen
        indexComplete
        askedFirstQuestion
        inspectedSource
        onOpenProject={vi.fn()}
        onRunIndex={vi.fn()}
        onSeedQuestion={vi.fn()}
        onInspectSource={vi.fn()}
      />,
    );

    expect(screen.getAllByText("Ready")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /^Ready$/i })).toBeNull();
  });
});
