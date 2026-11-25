import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors } from "./helpers";

// The screens instead of the wallet when its data can't be opened: a refused store (the engine's own
// screen), a data folder others can change, and StorageInUse. Neither falls through to onboarding, an empty wallet list or a zero balance.

const appExits = (page: Page) => page.evaluate(() => window.__dwFixture!.world.appExits ?? []);

async function expectNoWallet(page: Page) {
  for (const id of ["onboarding-welcome", "onboarding-create", "hero", "history-empty", "wallets-error", "toolbar-lock"]) await expect(page.getByTestId(id)).toHaveCount(0);
}

test.describe("a refused store", () => {
  for (const scenario of ["funded", "no-wallet"])
    test(`shows the engine's screen and quits, never onboarding (${scenario})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl(scenario, "light", "&openRefused=cantOpenWithoutBackup"));
      const screen = page.getByTestId("store-refused");
      await expect(screen.getByRole("heading")).toHaveText("Dash Wallet can't open this wallet data");
      await expect(page.getByTestId("store-refused-text")).toContainText("/home/user/.local/share/dashwallet/testnet. Nothing in it was changed. Your funds");
      await expectNoWallet(page);
      await page.getByTestId("store-refused-button-storeUpgrade.cantOpen.quit").click();
      await expect.poll(() => appExits(page)).toEqual(["quit"]);
      await expect(page.getByTestId("store-refused-problem")).toHaveCount(0);
      expect(errors).toEqual([]);
    });

  test("names the backup folder when a verified backup exists", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "dark", "&openRefused=cantOpen"));
    await expect(page.getByTestId("store-refused-text")).toContainText("is still in the upgrade-backups folder inside /home/user/.local/share/dashwallet/testnet");
    await expectNoWallet(page);
  });
});

test.describe("StorageInUse", () => {
  test("Try again opens the wallet once the session before lets go", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&storageInUse=retry"));
    await expect(page.getByTestId("storage-in-use-text")).toContainText("Try again in a moment, or restart Dash Wallet to finish.");
    await expectNoWallet(page);
    await page.getByTestId("storage-in-use-retry").click();
    await page.locator("[data-testid=hero], [data-testid=lock-screen]").first().waitFor();
    await expect(page.getByTestId("storage-in-use")).toHaveCount(0);
  });

  for (const scenario of ["funded", "no-wallet"])
    test(`only a restart frees a store held until one, never onboarding (${scenario})`, async ({ page }) => {
      await page.goto(fixtureUrl(scenario, "light", "&storageInUse=restart"));
      await expect(page.getByTestId("storage-in-use-text")).toHaveText(
        "Dash Wallet hasn't finished closing the wallet data it had open before, so it can't open it again yet. Your wallets and funds aren't affected. Restart Dash Wallet to finish.",
      );
      await expect(page.getByTestId("storage-in-use-retry")).toHaveCount(0);
      await expectNoWallet(page);
      await page.getByTestId("storage-in-use-restart").click();
      await page.locator("[data-testid=hero], [data-testid=lock-screen], [data-testid=onboarding-welcome]").first().waitFor();
      await expect(page.getByTestId("storage-in-use")).toHaveCount(0);
    });
});

test.describe("a data folder others can change", () => {
  const ROOT = "/home/user/.local/share/dashwallet";
  const button = (page: Page, k: string) => page.getByTestId(`store-not-private-button-storeUpgrade.notPrivate.${k}`);

  for (const scenario of ["funded", "no-wallet"])
    test(`names the folder and why, and Try again opens it only once it is private, never onboarding (${scenario})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl(scenario, "light", "&notPrivate=groupWritable"));
      const screen = page.getByTestId("store-not-private");
      await expect(screen.getByRole("heading")).toHaveText("Dash Wallet can't use this data folder");
      await expect(page.getByTestId("store-not-private-text")).toHaveText(
        `Other accounts on this computer can change files in ${ROOT}. To keep your wallet safe, Dash Wallet won't open it until only your account can change that folder.`,
      );
      await expect(page.getByTestId("store-not-private-reason")).toHaveText("Your group can write to it.");
      await expect(page.getByTestId("store-not-private-fix")).toHaveText("Change the folder's permissions so only you can write to it, then try again. Nothing in the folder was changed.");
      await expect(button(page, "retry")).toBeFocused();
      await expectNoWallet(page);

      const opens = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "open_network").length);
      const before = await opens();
      await button(page, "retry").click();
      await expect.poll(opens).toBe(before + 1);
      await expect(button(page, "retry")).toBeEnabled();
      await expect(screen).toBeVisible();
      await expectNoWallet(page);

      await page.evaluate(() => (window.__dwFixture!.world.openNotPrivate = undefined));
      await button(page, "retry").click();
      await page.locator("[data-testid=hero], [data-testid=lock-screen], [data-testid=onboarding-welcome]").first().waitFor();
      await expect(screen).toHaveCount(0);
      expect(errors).toEqual([]);
    });

  for (const [reason, line] of [
    ["otherWritable", "Every account can write to it."],
    ["otherOwner", "Another account owns it."],
  ] as const)
    test(`says why for ${reason}, reports a problem and quits`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("funded", "dark", `&notPrivate=${reason}`));
      await expect(page.getByTestId("store-not-private-reason")).toHaveText(line);
      await expect(page.getByTestId("store-not-private-hint")).toBeVisible();
      await button(page, "report").click();
      await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.linksOpened ?? [])).toEqual(["issues"]);
      await button(page, "quit").click();
      await expect.poll(() => appExits(page)).toEqual(["quit"]);
      await expect(page.getByTestId("store-not-private-problem")).toHaveCount(0);
      await expectNoWallet(page);
      expect(errors).toEqual([]);
    });
});
