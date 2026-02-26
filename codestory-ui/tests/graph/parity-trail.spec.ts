import { expect, test, type Locator, type Page } from "@playwright/test";

import { buildParityTrailScene, type ParitySceneVariant } from "./parityTrailScene";

const SNAPSHOT_OPTIONS = { maxDiffPixelRatio: 0.012 };

async function hoverBundledEdges(
  page: Page,
  workspace: Locator,
  bundledEdgeIds: string[],
): Promise<void> {
  for (const edgeId of bundledEdgeIds) {
    const edge = page.locator(`[data-edge-id="${edgeId}"]`);
    await edge.dispatchEvent("mouseenter");
    await expect(workspace).toHaveAttribute("data-hovered-edge-id", edgeId);
    await expect(edge).toHaveClass(/\bhovered\b/);
  }
}

async function selectBundledEdges(
  page: Page,
  workspace: Locator,
  bundledEdgeIds: string[],
): Promise<void> {
  for (const edgeId of bundledEdgeIds) {
    const edge = page.locator(`[data-edge-id="${edgeId}"]`);
    await edge.dispatchEvent("click");
    await expect(workspace).toHaveAttribute("data-selected-edge-id", edgeId);
    await expect(edge).toHaveClass(/\bselected\b/);
    await expect(page.locator(".edge-group.selected")).toHaveCount(1);
  }
}

async function runSceneSnapshotTest(
  page: Page,
  variant: ParitySceneVariant,
  snapshotPrefix: string,
): Promise<void> {
  await page.setViewportSize({ width: 1400, height: 900 });
  const scene = buildParityTrailScene(variant);
  expect(scene.bundledEdgeIds.length).toBeGreaterThan(0);

  await page.setContent(scene.html);
  const workspace = page.getByTestId("graph-workspace");
  await expect(page.locator('.edge-group[data-route-kind="flow-trunk"]')).toHaveCount(
    scene.bundledEdgeIds.length,
  );

  await expect(workspace).toHaveScreenshot(`${snapshotPrefix}.png`, SNAPSHOT_OPTIONS);
  await hoverBundledEdges(page, workspace, scene.bundledEdgeIds);
  await expect(workspace).toHaveScreenshot(`${snapshotPrefix}-hover.png`, SNAPSHOT_OPTIONS);

  await selectBundledEdges(page, workspace, scene.bundledEdgeIds);
  await expect(workspace).toHaveScreenshot(`${snapshotPrefix}-selected.png`, SNAPSHOT_OPTIONS);
}

test.describe("graph parity visual snapshots", () => {
  test("captures horizontal parity workspace with hover and selection interactions", async ({
    page,
  }) => {
    await runSceneSnapshotTest(page, "horizontal", "parity-horizontal");
  });

  test("captures vertical parity workspace with hover and selection interactions", async ({
    page,
  }) => {
    await runSceneSnapshotTest(page, "vertical", "parity-vertical");
  });
});
