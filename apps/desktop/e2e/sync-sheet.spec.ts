import { expect, test } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, THEMES } from "./helpers";

// Sync progress (IOS-023): the status bar's sync item opens a sheet with each phase's own progress.

test("a quiet stall is still called out: no further event after the engine's one-time reading", async ({ page }) => {
  await page.clock.install();
  await page.goto(fixtureUrl("empty-syncing", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => {
    const fx = window.__dwFixture!;
    fx.world.sync = { ...fx.world.sync, secondsSinceProgress: 45 };
    fx.emit({ kind: "sync", network: fx.world.launch.network.dirName, status: fx.world.sync });
  });
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-sheet-phases")).toBeVisible();
  await expect(page.getByTestId("sync-sheet-stalled")).toHaveCount(0);
  await page.clock.runFor(20_000);
  await expect(page.getByTestId("sync-sheet-stalled")).toContainText("No progress for 1 minute");
});

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

// Every sync the user waits on: the bar names the one still running, Sync progress lists each, and a
// Shielded account that waits for an unlock is Synced in the bar while its row says why.
test("the bar says Syncing Shielded and Sync progress lists each sync", async ({ page }) => {
  await page.goto(fixtureUrl("shielded-syncing", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("status-state")).toHaveText("Syncing Shielded…");
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-sheet-state")).toHaveText("Syncing Shielded…");
  await expect(page.getByTestId("sync-item-core")).toContainText("Done");
  await expect(page.getByTestId("sync-item-platform")).toContainText("Done");
  await expect(page.getByTestId("sync-item-shielded")).toContainText("42.0% scanned");
});

test("a Shielded account waiting for an unlock is Synced in the bar and says why in Sync progress", async ({ page }) => {
  await page.goto(fixtureUrl("shielded-locked", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("status-state")).toHaveText("Up to date");
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-item-shielded")).toContainText("Unlock to sync");
});

test("a failed Shielded pass is never Synced, and the bar clears once a pass succeeds", async ({ page }) => {
  await page.goto(fixtureUrl("shielded-pass-failed", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("status-state")).toHaveText("Shielded sync didn't finish");
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-item-shielded")).toContainText("The last pass failed. It tries again shortly.");
});

test("a failed Platform-address pass of the wallet is never Synced", async ({ page }) => {
  await page.goto(fixtureUrl("platform-addresses-failed", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("status-state")).toHaveText("Platform sync didn't finish");
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-item-platform")).toContainText("The last pass failed. It tries again shortly.");
});

test("a Shielded sync status that cannot be read is never Synced", async ({ page }) => {
  await page.goto(fixtureUrl("shielded-sync-unread", "light"));
  await page.getByTestId("toolbar").waitFor();
  await expect(page.getByTestId("status-state")).toHaveText("Couldn't check Shielded's sync");
  await page.getByTestId("status-sync").click();
  await expect(page.getByTestId("sync-item-shielded")).toContainText("Couldn't check this sync. It tries again shortly.");
});
