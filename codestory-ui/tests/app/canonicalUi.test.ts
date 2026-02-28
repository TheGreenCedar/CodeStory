import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

describe("canonical ui without feature flags", () => {
  it("does not reference feature-flag keys in App", () => {
    const appPath = join(process.cwd(), "src", "App.tsx");
    const source = readFileSync(appPath, "utf8");
    expect(source).not.toContain("uxResetV2");
    expect(source).not.toContain("singlePaneInvestigate");
    expect(source).not.toContain("spacesLibrary");
    expect(source).not.toContain("onboardingStarter");
  });

  it("does not render settings toggles anymore", () => {
    const settingsPath = join(process.cwd(), "src", "features", "settings", "SettingsPage.tsx");
    const source = readFileSync(settingsPath, "utf8");
    expect(source).not.toContain('type="checkbox"');
    expect(source).toContain("Playful Geometric theme");
  });
});
