import { expect, test } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, THEMES } from "./helpers";

// Sync progress (IOS-023): the status bar's sync item opens a sheet with each phase's own progress.

for (const theme of THEMES)
  test(`the status bar's sync item shows each phase, ${theme}`, async ({ page }, info) => {
    await page.goto(fixtureUrl("empty-syncing", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("status-sync").click();
    const sheet = page.getByTestId("sync-sheet");
    await expect(sheet).toBeVisible();
    await expect(page.getByTestId("sync-phase-headers")).toContainText("Done");
    await expect(page.getByTestId("sync-phase-filters")).toContainText("(60.0%)");
    await expect(page.getByTestId("sync-phase-masternodes")).toContainText("0 of");
    await expect(page.getByTestId("sync-peers")).toBeVisible();
    await page.evaluate(() => {
      const fx = window.__dwFixture!;
      fx.world.sync = { ...fx.world.sync, secondsSinceProgress: 180 };
      fx.emit({ kind: "sync", network: fx.world.launch.network.dirName, status: fx.world.sync });
    });
    await expect(page.getByTestId("sync-sheet-stalled")).toContainText("No progress for 3 minutes", { timeout: 15_000 });
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `sync-sheet-${theme}.png`) });
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
  });
