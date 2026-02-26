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

  it("ignores legacy bundling mode from persisted layouts", () => {
    const normalized = normalizeTrailUiConfig({
      depth: 3,
      // Legacy field from earlier UI schema.
      bundlingMode: "trace",
    } as unknown as Parameters<typeof normalizeTrailUiConfig>[0]);

    expect(normalized.depth).toBe(3);
    expect("bundlingMode" in normalized).toBe(false);
  });
});
