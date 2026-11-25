import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors } from "./helpers";

// The screens instead of the wallet when its data can't be opened: a refused store (the engine's own
// screen) and StorageInUse. Neither falls through to onboarding, an empty wallet list or a zero balance.

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
