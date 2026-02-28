import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AppShell } from "../../src/layout/AppShell";

describe("AppShell", () => {
  it("renders only investigate, library, settings sections", () => {
    const onSelect = vi.fn();
    render(
      <AppShell
        activeSection="investigate"
        onSelectSection={onSelect}
        workspace={<div>workspace</div>}
        sectionContent={{
          library: <div>library</div>,
          settings: <div>settings</div>,
        }}
      />,
    );

    expect(screen.getByRole("button", { name: /Investigate/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Library/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Settings/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Home/i })).toBeNull();
  });

  it("switches section via nav buttons", () => {
    const onSelect = vi.fn();
    render(
      <AppShell
        activeSection="investigate"
        onSelectSection={onSelect}
        workspace={<div>workspace</div>}
        sectionContent={{
          library: <div>library</div>,
          settings: <div>settings</div>,
        }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Library/i }));
    expect(onSelect).toHaveBeenCalledWith("library");
  });
});
