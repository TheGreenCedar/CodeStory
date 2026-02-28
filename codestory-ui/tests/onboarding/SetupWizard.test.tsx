import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  SetupWizard,
  type SetupWizardIndexProgress,
  type SetupWizardProps,
} from "../../src/features/onboarding/SetupWizard";

function renderWizard(overrides: Partial<SetupWizardProps> = {}) {
  const props: SetupWizardProps = {
    projectPath: "C:\\Users\\alber\\source\\repos\\codestory",
    onProjectPathChange: vi.fn(),
    projectOpen: false,
    indexProgress: null,
    onOpenProject: vi.fn(),
    onIndex: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };

  const view = render(<SetupWizard {...props} />);
  return {
    ...view,
    props,
  };
}

describe("SetupWizard", () => {
  it("renders three steps with incremental mode recommended by default", () => {
    renderWizard({
      messageSlot: <p>Set a valid path to continue.</p>,
    });

    expect(screen.getByText("1. Confirm project path")).toBeInTheDocument();
    expect(screen.getByText("2. Choose index mode")).toBeInTheDocument();
    expect(screen.getByText("3. Check readiness")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Incremental \(recommended\)/i })).toBeChecked();
    expect(
      screen.getByText(
        /Recommended for everyday work\. Incremental indexing updates only what changed/i,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Set a valid path to continue.")).toBeInTheDocument();
  });

  it("wires project path and open project actions", () => {
    const onProjectPathChange = vi.fn();
    const onOpenProject = vi.fn();

    renderWizard({
      projectPath: ".",
      onProjectPathChange,
      onOpenProject,
    });

    fireEvent.change(screen.getByLabelText("Project path"), {
      target: { value: "C:\\code\\workspace" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Open project" }));

    expect(onProjectPathChange).toHaveBeenCalledWith("C:\\code\\workspace");
    expect(onOpenProject).toHaveBeenCalledTimes(1);
  });

  it("calls onIndex with selected mode and shows indexing progress", () => {
    const onIndex = vi.fn();
    const indexProgress: SetupWizardIndexProgress = { current: 2, total: 10 };

    renderWizard({
      projectOpen: true,
      onIndex,
      indexProgress,
    });

    fireEvent.click(screen.getByRole("button", { name: "Start Incremental index" }));
    fireEvent.click(screen.getByRole("radio", { name: "Full index" }));
    fireEvent.click(screen.getByRole("button", { name: "Start Full index" }));

    expect(onIndex).toHaveBeenNthCalledWith(1, "Incremental");
    expect(onIndex).toHaveBeenNthCalledWith(2, "Full");
    expect(screen.getByText("Indexing 2 of 10 files.")).toBeInTheDocument();
    expect(screen.getByText("20% complete")).toBeInTheDocument();
  });

  it("enables finishing when readiness is met and closes via callback", () => {
    const onClose = vi.fn();

    renderWizard({
      projectOpen: true,
      isReady: true,
      onClose,
    });

    expect(
      screen.getByText("Ready to continue. You can ask your first question now."),
    ).toBeInTheDocument();
    const finishButton = screen.getByRole("button", { name: "Finish setup" });
    expect(finishButton).toBeEnabled();

    fireEvent.click(finishButton);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
