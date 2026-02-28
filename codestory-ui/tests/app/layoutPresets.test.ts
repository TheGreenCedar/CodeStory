import { describe, expect, it } from "vitest";

import {
  investigateFocusModeLabel,
  migrateLegacyWorkspacePreset,
  normalizeInvestigateFocusMode,
} from "../../src/layout/layoutPresets";

describe("layoutPresets", () => {
  it("normalizes current and legacy focus values", () => {
    expect(normalizeInvestigateFocusMode("ask")).toBe("ask");
    expect(normalizeInvestigateFocusMode("graph")).toBe("graph");
    expect(normalizeInvestigateFocusMode("code")).toBe("code");
    expect(normalizeInvestigateFocusMode("learn")).toBe("graph");
    expect(normalizeInvestigateFocusMode("debug")).toBe("graph");
    expect(normalizeInvestigateFocusMode("review")).toBe("code");
    expect(normalizeInvestigateFocusMode("unknown")).toBe("graph");
  });

  it("migrates legacy workspace presets", () => {
    expect(migrateLegacyWorkspacePreset("learn")).toBe("graph");
    expect(migrateLegacyWorkspacePreset("debug")).toBe("graph");
    expect(migrateLegacyWorkspacePreset("review")).toBe("code");
    expect(migrateLegacyWorkspacePreset("bad")).toBeNull();
  });

  it("returns readable labels", () => {
    expect(investigateFocusModeLabel("ask")).toBe("Ask");
    expect(investigateFocusModeLabel("graph")).toBe("Graph");
    expect(investigateFocusModeLabel("code")).toBe("Code");
  });
});
