import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  Checklist,
  checklistStorageKey,
  type ChecklistProps,
} from "../../src/features/onboarding/Checklist";

function renderChecklist(overrides: Partial<ChecklistProps> = {}) {
  const props: ChecklistProps = {
    projectPath: "C:\\Users\\alber\\source\\repos\\codestory",
    projectOpen: false,
    indexComplete: false,
    askedFirstQuestion: false,
    inspectedSource: false,
    onOpenProject: vi.fn(),
    onIndex: vi.fn(),
    onAskFirstQuestion: vi.fn(),
    onInspectSource: vi.fn(),
    ...overrides,
  };

  const view = render(<Checklist {...props} />);
  return {
    ...view,
    props,
  };
}

describe("Checklist", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("renders onboarding items and calls action callbacks", () => {
    const onOpenProject = vi.fn();
    const onIndex = vi.fn();
    const onInspectSource = vi.fn();

    const { rerender } = renderChecklist({
      projectOpen: false,
      onOpenProject,
      onIndex,
      onAskFirstQuestion: vi.fn(),
      onInspectSource,
    });

    expect(screen.getByText("First-run checklist")).toBeInTheDocument();
    expect(screen.getByText("0 of 4 steps complete.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open project" }));
    expect(onOpenProject).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Run index" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Inspect source" })).toBeDisabled();

    rerender(
      <Checklist
        projectPath="C:\\Users\\alber\\source\\repos\\codestory"
        projectOpen
        indexComplete={false}
        askedFirstQuestion={false}
        inspectedSource={false}
        onOpenProject={onOpenProject}
        onIndex={onIndex}
        onAskFirstQuestion={vi.fn()}
        onInspectSource={onInspectSource}
      />,
    );

    expect(screen.getByText("1 of 4 steps complete.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run index" }));
    fireEvent.click(screen.getByRole("button", { name: "Inspect source" }));

    expect(onIndex).toHaveBeenCalledTimes(1);
    expect(onInspectSource).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Ask first question" })).toBeDisabled();
  });

  it("persists dismissal per project and supports reopen", () => {
    const projectPath = "C:\\repo\\alpha";
    const key = checklistStorageKey(projectPath);

    const { unmount } = renderChecklist({ projectPath });
    fireEvent.click(screen.getByRole("button", { name: "Dismiss checklist" }));

    expect(screen.getByRole("button", { name: "Reopen checklist" })).toBeInTheDocument();
    expect(window.localStorage.getItem(key)).toContain('"dismissed":true');

    unmount();

    renderChecklist({ projectPath });
    expect(screen.getByRole("button", { name: "Reopen checklist" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Reopen checklist" }));
    expect(screen.getByText("First-run checklist")).toBeInTheDocument();
    expect(window.localStorage.getItem(key)).toBeNull();
  });

  it("uses a namespaced key so each project has independent checklist visibility", () => {
    const firstPath = "C:\\repo\\first";
    const secondPath = "C:\\repo\\second";
    const firstKey = checklistStorageKey(firstPath);
    const secondKey = checklistStorageKey(secondPath);

    const { rerender } = renderChecklist({ projectPath: firstPath });
    fireEvent.click(screen.getByRole("button", { name: "Dismiss checklist" }));
    expect(window.localStorage.getItem(firstKey)).toContain('"dismissed":true');

    rerender(
      <Checklist
        projectPath={secondPath}
        projectOpen={false}
        indexComplete={false}
        askedFirstQuestion={false}
        inspectedSource={false}
        onOpenProject={vi.fn()}
        onIndex={vi.fn()}
        onAskFirstQuestion={vi.fn()}
        onInspectSource={vi.fn()}
      />,
    );

    expect(screen.getByText("First-run checklist")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reopen checklist" })).not.toBeInTheDocument();
    expect(window.localStorage.getItem(secondKey)).toBeNull();
  });
});
