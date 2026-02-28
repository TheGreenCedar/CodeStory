import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  CommandPalette,
  rankCommands,
  type CommandPaletteCommand,
} from "../src/components/CommandPalette";

const COMMANDS: CommandPaletteCommand[] = [
  {
    id: "open-project",
    label: "Open Project",
    keywords: ["workspace", "setup"],
    run: () => undefined,
  },
  {
    id: "full-index",
    label: "Run Full Index",
    keywords: ["index", "rebuild"],
    run: () => undefined,
  },
  {
    id: "trail",
    label: "Run Trail Query",
    keywords: ["graph", "path"],
    run: () => undefined,
  },
];

describe("CommandPalette", () => {
  it("prioritizes strong label matches in ranking", () => {
    const ranked = rankCommands(COMMANDS, "open");

    expect(ranked).toHaveLength(1);
    expect(ranked[0]?.id).toBe("open-project");
  });

  it("invokes the active command when Enter is pressed", () => {
    const run = vi.fn();
    const onClose = vi.fn();

    render(
      <CommandPalette
        open
        onClose={onClose}
        commands={[
          {
            id: "open-project",
            label: "Open Project",
            run,
          },
        ]}
      />,
    );

    fireEvent.keyDown(window, { key: "Enter" });

    expect(run).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("filters list based on search terms", () => {
    render(<CommandPalette open onClose={vi.fn()} commands={COMMANDS} />);

    fireEvent.change(screen.getByLabelText("Search commands"), {
      target: { value: "trail" },
    });

    expect(screen.getByRole("option", { name: /Run Trail Query/i })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Open Project/i })).not.toBeInTheDocument();
  });
});
