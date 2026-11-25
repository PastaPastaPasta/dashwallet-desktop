import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { fixtureUrl, THEMES, trackPageErrors } from "./helpers";

// The restore of the wallet data from before the upgrade: a restore that stopped the open (each state),
// the foreign lineage's Keep the current files, Restore earlier data through its confirm and a restart,
// and the restored banner. None falls through to onboarding, an empty wallet list or a zero balance.

const FOLDER = "/home/user/.local/share/dashwallet/testnet";
const world = (page: Page) => page.evaluate(() => window.__dwFixture!.world);
const calls = (page: Page) => page.evaluate(() => [...window.__dwFixture!.calls]);

async function expectNoWallet(page: Page) {
  for (const id of ["onboarding-welcome", "onboarding-create", "hero", "history-empty", "wallets-error", "toolbar-lock"]) await expect(page.getByTestId(id)).toHaveCount(0);
}

/** The wallet behind the lock screen a restart leaves, unlocked. */
async function unlockIfLocked(page: Page) {
  await page.locator("[data-testid=hero], [data-testid=lock-screen]").first().waitFor();
  if ((await page.getByTestId("lock-screen").count()) === 0) return;
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("lock-field").press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}

const STATES = [
  { name: "refused:missingBackup", title: "The earlier wallet data couldn't be restored", buttons: ["ok"] },
  { name: "noSpace:diskFull", title: "Not enough disk space to restore", buttons: ["ok"] },
  { name: "notStarted:permissionDenied", title: "Dash Wallet couldn't start restoring your wallet data", buttons: ["restart", "cancel", "report", "quit"] },
  { name: "incomplete:diskFull", title: "Dash Wallet couldn't finish restoring your wallet data", buttons: ["restart", "report", "quit"] },
];

test.describe("a restore that stopped the open", () => {
  for (const theme of THEMES)
    for (const { name, title, buttons } of STATES)
      test(`shows ${name}'s screen, never onboarding (${theme})`, async ({ page }) => {
        const errors = trackPageErrors(page);
        await page.goto(fixtureUrl("funded", theme, `&openRestore=${name}`));
        const screen = page.getByTestId("store-restore");
        await expect(screen.getByRole("heading")).toHaveText(title);
        await expect(screen.getByRole("button")).toHaveCount(buttons.length);
        const state = name.split(":")[0];
        for (const b of buttons) await expect(page.getByTestId(`store-restore-button-storeUpgrade.restore.${state}.${b}`)).toBeEnabled();
        await expect(page.getByTestId("store-restore-hint")).toHaveCount(buttons.includes("report") ? 1 : 0);
        await expect(page.getByTestId("store-restore-text")).not.toContainText("{folder}");
        await expectNoWallet(page);
        expect(errors).toEqual([]);
      });

  test("OK after a refused restore opens the wallet data as it was", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=refused:garbledMarker"));
    await page.getByTestId("store-restore-button-storeUpgrade.restore.refused.ok").click();
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("store-restore")).toHaveCount(0);
    await expect(page.getByTestId("restored-banner")).toHaveCount(0);
  });

  test("Cancel restore drops the request, and the current data opens", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "dark", "&openRestore=notStarted:fileVanished"));
    await expect(page.getByTestId("store-restore-text")).toContainText(`can read and write ${FOLDER}`);
    await page.getByTestId("store-restore-button-storeUpgrade.restore.notStarted.cancel").click();
    await page.getByTestId("hero").waitFor();
    expect((await world(page)).restoreCalls).toEqual(["cancelRestore"]);
  });

  test("notStarted's Cancel restore calls cancel_restore once and nothing else before the open again", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=notStarted:permissionDenied"));
    await page.getByTestId("store-restore").waitFor();
    const before = (await calls(page)).length;
    await page.getByTestId("store-restore-button-storeUpgrade.restore.notStarted.cancel").click();
    await unlockIfLocked(page);
    const after = (await calls(page)).slice(before);
    expect(after.slice(0, after.indexOf("open_network"))).toEqual(["cancel_restore"]);
    const w = await world(page);
    expect(w.restoreCalls).toEqual(["cancelRestore"]);
    expect(w.appExits ?? []).toEqual([]);
    expect(w.linksOpened ?? []).toEqual([]);
  });

  test("notStarted's Report a problem opens the issues link, and nothing else", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=notStarted:permissionDenied"));
    await page.getByTestId("store-restore").waitFor();
    const before = (await calls(page)).length;
    await page.getByTestId("store-restore-button-storeUpgrade.restore.notStarted.report").click();
    await expect.poll(async () => (await world(page)).linksOpened).toEqual(["issues"]);
    expect((await calls(page)).slice(before)).toEqual(["open_project_link"]);
    const w = await world(page);
    expect(w.restoreCalls ?? []).toEqual([]);
    expect(w.appExits ?? []).toEqual([]);
    await expect(page.getByTestId("store-restore")).toBeVisible();
  });

  test("Restart on a restore that stopped partway starts again and still shows it", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=incomplete:other"));
    await page.getByTestId("store-restore-button-storeUpgrade.restore.incomplete.restart").click();
    await page.getByTestId("store-restore").waitFor();
    expect((await world(page)).appExits).toEqual(["restart"]);
    await expectNoWallet(page);
  });

  test("Report a problem opens the issues page", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=incomplete:permissionDenied"));
    await page.getByTestId("store-restore-button-storeUpgrade.restore.incomplete.report").click();
    await expect.poll(async () => (await world(page)).linksOpened).toEqual(["issues"]);
  });
});

test.describe("the foreign lineage", () => {
  for (const theme of THEMES)
    test(`Keep the current files keeps them and opens them, restored (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("funded", theme, "&openRestore=foreign:otherLineage"));
      const screen = page.getByTestId("store-restore");
      await expect(screen.getByRole("heading")).toHaveText("These wallet files don't match the restore");
      await expect(page.getByTestId("store-restore-hint")).toContainText(`opens the files in ${FOLDER} as they are now`);
      await expect(page.getByTestId("store-restore-issues-hint")).toHaveText("Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.");
      const keep = page.getByTestId("store-restore-button-storeUpgrade.restore.foreign.keep");
      await expect(keep).not.toBeFocused();
      await expectNoWallet(page);
      await keep.click();
      await unlockIfLocked(page);
      await expect(page.getByTestId("restored-banner-paused")).toHaveText("Anything that moves your funds is paused until syncing finishes.");
      expect((await world(page)).restoreCalls).toEqual(["keepCurrentFiles"]);
      expect(errors).toEqual([]);
    });

  test("Quit quits through its own key, keeping nothing", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRestore=foreign:otherLineage"));
    await expect(page.getByTestId("store-refused")).toHaveCount(0);
    const quit = page.getByTestId("store-restore-button-storeUpgrade.restore.foreign.quit");
    await expect(quit).toHaveText("Quit");
    await quit.click();
    await expect.poll(async () => (await world(page)).appExits).toEqual(["quit"]);
    expect((await world(page)).restoreCalls).toBeUndefined();
    await expectNoWallet(page);
  });
});

test.describe("Restore earlier data", () => {
  test("shows only while a verified snapshot is there", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRefused=refusedRestore&restoreAvailable=no"));
    await page.getByTestId("store-refused").waitFor();
    await expect.poll(async () => (await world(page)).restoreCalls).toEqual(["restoreAvailable"]);
    await expect(page.getByTestId("store-refused-button-storeUpgrade.refused.restore")).toHaveCount(0);
  });

  test("Cancel in the confirm asks for nothing", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "dark", "&openRefused=refusedRestore&restoreAvailable=yes"));
    await page.getByTestId("store-refused-button-storeUpgrade.refused.restore").click();
    const confirm = page.getByTestId("restore-confirm");
    await expect(confirm.getByRole("heading")).toHaveText("Restore wallet data from before the upgrade?");
    await expect(page.getByTestId("restore-confirm-cancel")).toBeFocused();
    await page.getByTestId("restore-confirm-cancel").click();
    await expect(confirm).toHaveCount(0);
    expect((await world(page)).restoreCalls).toEqual(["restoreAvailable"]);
    await expectNoWallet(page);
  });

  for (const theme of THEMES)
    test(`Restart and restore asks for the restore and restarts; the earlier data opens with the banner (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("funded", theme, "&openRefused=refusedRestore&restoreAvailable=yes"));
      await page.getByTestId("store-refused-button-storeUpgrade.refused.restore").click();
      await expect(page.getByTestId("restore-confirm-text").first()).toContainText(`upgrade-backups folder inside ${FOLDER}`);
      await page.getByTestId("restore-confirm-confirm").click();
      await unlockIfLocked(page);
      await expect(page.getByTestId("store-refused")).toHaveCount(0);
      await expect(page.getByTestId("restored-banner-text")).toContainText("Dash Wallet is using your wallet data from before the upgrade");
      const after = await world(page);
      expect(after.restoreCalls).toEqual(["restoreAvailable", "requestRestore"]);
      expect(after.appExits).toEqual(["restart"]);
      expect(errors).toEqual([]);
    });
});

test.describe("the restored banner", () => {
  test("shows while RESTORED is set, and not otherwise", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&restored=yes"));
    await expect(page.getByTestId("restored-banner")).toBeVisible();
    await page.goto(fixtureUrl("funded", "light"));
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("restored-banner")).toHaveCount(0);
  });
});
