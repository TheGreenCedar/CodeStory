import { describe, expect, it } from "vitest";

import { defaultTrailUiConfig, normalizeTrailUiConfig } from "../../src/graph/trailConfig";

describe("trailConfig", () => {
  it("defaults trail depth to 1", () => {
    expect(defaultTrailUiConfig().depth).toBe(1);
    expect(normalizeTrailUiConfig(undefined).depth).toBe(1);
  });

  it("falls back to depth 1 for invalid persisted depth", () => {
    expect(normalizeTrailUiConfig({ depth: Number.NaN }).depth).toBe(1);
  });
});
