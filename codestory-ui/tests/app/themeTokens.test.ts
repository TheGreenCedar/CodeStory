import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { EDGE_KIND_COLORS, GRAPH_THEME, PLAYFUL_TOKENS } from "../../src/theme/tokens";

describe("theme token contract", () => {
  it("exposes the Playful Geometric core palette and radii", () => {
    expect(PLAYFUL_TOKENS.colors.background).toBe("#FFFDF5");
    expect(PLAYFUL_TOKENS.colors.accent).toBe("#8B5CF6");
    expect(PLAYFUL_TOKENS.radius.md).toBe("16px");
    expect(PLAYFUL_TOKENS.motion.bounce).toContain("cubic-bezier");
  });

  it("maps graph semantics to stable edge colors", () => {
    expect(EDGE_KIND_COLORS.CALL).toBeDefined();
    expect(EDGE_KIND_COLORS.INHERITANCE).toBeDefined();
    expect(EDGE_KIND_COLORS.UNKNOWN).toBeDefined();
    expect(GRAPH_THEME.minimap.background).toContain("rgb");
  });

  it("defines css variables in tokens.css", () => {
    const cssPath = join(process.cwd(), "src", "theme", "tokens.css");
    const css = readFileSync(cssPath, "utf8");
    expect(css).toContain("--color-background");
    expect(css).toContain("--shadow-pop");
    expect(css).toContain("--graph-canvas-top");
  });
});
