import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/graph",
  testMatch: /parity-trail\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: "list",
  outputDir: "tests/graph/.playwright-artifacts",
  use: {
    headless: true,
    viewport: { width: 1400, height: 900 },
    colorScheme: "light",
  },
});
