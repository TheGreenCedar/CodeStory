import { describe, expect, it } from "vitest";

import {
  TRAIL_PERSPECTIVE_PRESETS,
  defaultTrailUiConfig,
  normalizeTrailUiConfig,
  trailConfigFromPerspectivePreset,
  trailPerspectivePresetForConfig,
} from "../../src/graph/trailConfig";

describe("trailConfig", () => {
  it("defaults trail depth to 1", () => {
    expect(defaultTrailUiConfig().depth).toBe(1);
    expect(normalizeTrailUiConfig(undefined).depth).toBe(1);
  });

  it("falls back to depth 1 for invalid persisted depth", () => {
    expect(normalizeTrailUiConfig({ depth: Number.NaN }).depth).toBe(1);
  });

  it("ignores legacy bundling mode from persisted layouts", () => {
    const normalized = normalizeTrailUiConfig({
      depth: 3,
      // Legacy field from earlier UI schema.
      bundlingMode: "trace",
    } as unknown as Parameters<typeof normalizeTrailUiConfig>[0]);

    expect(normalized.depth).toBe(3);
    expect("bundlingMode" in normalized).toBe(false);
  });

  it("provides deterministic defaults for each perspective preset", () => {
    for (const preset of TRAIL_PERSPECTIVE_PRESETS) {
      const first = trailConfigFromPerspectivePreset(preset);
      const second = trailConfigFromPerspectivePreset(preset);
      expect(first).toEqual(second);
      expect(first.edgeFilter).not.toBe(second.edgeFilter);
      expect(first.nodeFilter).not.toBe(second.nodeFilter);
      expect(first.targetId).toBeNull();
      expect(first.targetLabel).toBe("");
    }
  });

  it("maps call flow preset to outgoing trail defaults", () => {
    const preset = trailConfigFromPerspectivePreset("CallFlow");

    expect(preset.mode).toBe("Neighborhood");
    expect(preset.direction).toBe("Outgoing");
    expect(preset.layoutDirection).toBe("Vertical");
    expect(preset.depth).toBe(4);
    expect(preset.edgeFilter).toEqual(["CALL", "OVERRIDE", "MACRO_USAGE"]);
    expect(preset.nodeFilter).toEqual([
      "CLASS",
      "STRUCT",
      "INTERFACE",
      "FUNCTION",
      "METHOD",
      "MACRO",
    ]);
  });

  it("recognizes preset-matching configs regardless of filter order", () => {
    const preset = trailConfigFromPerspectivePreset("Impact");
    const shuffled = {
      ...preset,
      edgeFilter: [...preset.edgeFilter].reverse(),
      nodeFilter: [...preset.nodeFilter].reverse(),
    };

    expect(trailPerspectivePresetForConfig(shuffled)).toBe("Impact");
    expect(trailPerspectivePresetForConfig(defaultTrailUiConfig())).toBeNull();
  });
});
