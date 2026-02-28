import { describe, expect, it } from "vitest";

import { normalizeFeatureFlags } from "../../src/app/featureFlags";

describe("featureFlags", () => {
  it("uses defaults when payload missing", () => {
    expect(normalizeFeatureFlags(null)).toEqual({
      uxResetV2: true,
      onboardingStarter: true,
      singlePaneInvestigate: true,
      spacesLibrary: true,
    });
  });

  it("migrates legacy keys", () => {
    expect(
      normalizeFeatureFlags({ modernShell: false, onboarding: false, spacesLibrary: true }),
    ).toEqual({
      uxResetV2: false,
      onboardingStarter: false,
      singlePaneInvestigate: true,
      spacesLibrary: true,
    });
  });

  it("prefers v2 keys when present", () => {
    expect(
      normalizeFeatureFlags({
        modernShell: false,
        onboarding: false,
        uxResetV2: true,
        onboardingStarter: true,
        singlePaneInvestigate: false,
        spacesLibrary: false,
      }),
    ).toEqual({
      uxResetV2: true,
      onboardingStarter: true,
      singlePaneInvestigate: false,
      spacesLibrary: false,
    });
  });
});
