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
      await page.goto(fixtureUrl(scenario, "light", "&openRefused=refusedPrevious"));
      const screen = page.getByTestId("store-refused");
      await expect(screen.getByRole("heading")).toHaveText("Dash Wallet can't open this wallet data safely");
      await expect(page.getByTestId("store-refused-text")).toContainText("/home/user/.local/share/dashwallet/testnet: it holds contact data that this version can't read yet. Dash Wallet didn't change anything in it.");
      await expectNoWallet(page);
      await page.getByTestId("store-refused-button-storeUpgrade.refused.quit").click();
      await expect.poll(() => appExits(page)).toEqual(["quit"]);
      await expect(page.getByTestId("store-refused-problem")).toHaveCount(0);
      expect(errors).toEqual([]);
    });

  test("offers Report a problem when the way out is support", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "dark", "&openRefused=refusedSupport"));
    await expect(page.getByTestId("store-refused-text")).toContainText("Choose Report a problem.");
    await expect(page.getByTestId("store-refused-button-storeUpgrade.refused.report")).toHaveText("Report a problem");
    await expectNoWallet(page);
  });
});

test.describe("an upgrade that stopped on the machine", () => {
  for (const theme of ["light", "dark"] as const)
    test(`says to free up space and open again, never the earlier version, and quits (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("funded", theme, "&openRetry=yes"));
      const screen = page.getByTestId("store-retry");
      await expect(screen.getByRole("heading")).toHaveText("Dash Wallet couldn't finish upgrading your wallet data");
      await expect(page.getByTestId("store-retry-text")).toHaveText(
        "The disk is full, or Dash Wallet couldn't write to it. Free up some space, then quit Dash Wallet and open it again. Until then, don't open this wallet data with an earlier version.",
      );
      await expect(page.getByTestId("store-retry-button-storeUpgrade.refused.report")).toBeFocused();
      await expect(page.getByTestId("store-retry-hint")).toHaveText("Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.");
      await expectNoWallet(page);
      await page.getByTestId("store-retry-button-storeUpgrade.refused.quit").click();
      await expect.poll(() => appExits(page)).toEqual(["quit"]);
      expect(errors).toEqual([]);
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
      await expect(page.getByTestId("store-not-private-fix")).toHaveText("Change the folder's permissions so only you can write to it, then try again. Your wallet files weren't changed.");
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
