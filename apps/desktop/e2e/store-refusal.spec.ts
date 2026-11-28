import { expect, test, type Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { MAIN_PHRASE, SECOND_PHRASE } from "../src/fixture/vault";
import { fixtureUrl, THEMES, trackPageErrors } from "./helpers";

// The screens instead of the wallet when its data can't be opened: a refused store (the engine's own
// screen), a data folder others can change, and StorageInUse. Neither falls through to onboarding, an empty wallet list or a zero balance;
// Set up from recovery phrase leads to onboarding in a new folder after a restart.

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

  test("explains a retired backup and warns about in-progress work before recovery", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&openRefused=retiredBackup"));
    const screen = page.getByTestId("store-refused");
    const text = page.getByTestId("store-refused-text");
    await expect(text).toContainText("Dash Wallet retired the backup from before the upgrade as part of removing a wallet.");
    await expect(text).toContainText("Payment holds, in-progress operations, imported keys and other local records stay in that folder.");
    await expect(text).toContainText("If a payment, a governance proposal, a username or identity registration, a credit top-up or a masternode transaction was in progress, or you don't have a recovery phrase, choose Report a problem first.");
    await expect(screen.locator("button")).toHaveText(["Set up from recovery phrase", "Report a problem", "Quit"]);
    await page.getByTestId("store-refused-button-storeUpgrade.refused.recover").click();
    await expect(page.getByTestId("recover-confirm")).toBeVisible();
    await page.getByTestId("recover-confirm-cancel").click();
    await expect(page.getByTestId("recover-confirm")).toHaveCount(0);
    expect(await appExits(page)).toEqual([]);
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

test.describe("a damaged store (Q107 C1)", () => {
  const RECOVER = "store-refused-button-storeUpgrade.refused.recover";
  const restoreCalls = (page: Page) => page.evaluate(() => window.__dwFixture!.world.restoreCalls ?? []);
  const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
  const relaunch = (page: Page) => page.evaluate(() => window.__dwFixture!.relaunch());

  async function restorePhrase(page: Page, phrase: string) {
    await page.getByTestId("onboarding-restore").click();
    await page.getByTestId("restore-input").fill(phrase);
    await page.getByTestId("restore-continue").click();
    await page.getByTestId("restore-finish").click();
  }

  /** Welcome offers the phrase restore only: no new wallet, no file. */
  async function expectRestoreOnly(page: Page) {
    await expect(page.getByTestId("onboarding-welcome")).toBeVisible();
    await expect(page.getByTestId("onboarding-restore")).toBeVisible();
    for (const id of ["onboarding-create", "onboarding-restore-file", "onboarding-24-words"]) await expect(page.getByTestId(id)).toHaveCount(0);
  }

  for (const theme of THEMES)
    test(`offers the engine's Set up from recovery phrase first, unfocused, under its damaged-store text (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("funded", theme, "&openRefused=damaged"));
      const buttons = page.getByTestId("store-refused").locator("button");
      await expect(buttons).toHaveText(["Set up from recovery phrase", "Report a problem", "Quit"]);
      await expect(page.getByTestId(RECOVER)).not.toBeFocused();
      const text = page.getByTestId("store-refused-text");
      await expect(text).toContainText("it can't be read as wallet data, so it may be damaged.");
      await expect(text).toContainText("Dash Wallet leaves /home/user/.local/share/dashwallet/testnet as it is and doesn't delete anything.");
      await expectNoWallet(page);
      expect(errors).toEqual([]);
    });

  for (const screen of ["refusedPrevious", "refusedSupport", "refusedRestore"])
    test(`is not offered on any other refusal (${screen})`, async ({ page }) => {
      await page.goto(fixtureUrl("funded", "light", `&openRefused=${screen}&restoreAvailable=yes`));
      await expect(page.getByTestId("store-refused")).toBeVisible();
      await expect(page.getByTestId(RECOVER)).toHaveCount(0);
    });

  test("asks first, then sets up in a new folder after a restart: the phrase restore only, more wallets through Add a wallet, and never the refusal again", async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("funded", "light", "&openRefused=damaged"));

    // Cancel writes nothing.
    await page.getByTestId(RECOVER).click();
    await expect(page.getByTestId("recover-confirm").locator("h2")).toHaveText("Set up from a recovery phrase?");
    await expect(page.getByTestId("recover-confirm-text")).toHaveText(
      "Dash Wallet restarts with a new, empty data folder, and you enter a wallet's recovery phrase. It doesn't change or delete anything in /home/user/.local/share/dashwallet/testnet.",
    );
    await expect(page.getByTestId("recover-confirm-cancel")).toBeFocused();
    await page.getByTestId("recover-confirm-cancel").click();
    await expect(page.getByTestId("recover-confirm")).toHaveCount(0);
    expect(await restoreCalls(page)).toEqual([]);
    expect(await appExits(page)).toEqual([]);

    // Restart and set up: the new folder once, then the restart.
    await page.getByTestId(RECOVER).click();
    await page.getByTestId("recover-confirm-confirm").click();
    await expectRestoreOnly(page);
    expect(await restoreCalls(page)).toEqual(["recover"]);
    expect(await appExits(page)).toEqual(["restart"]);
    await expect(page.getByTestId("onboarding-welcome-text")).toHaveText(
      "Your earlier wallet data couldn't be read, so Dash Wallet started with a new data folder. Enter a recovery phrase to set up a wallet again.",
    );
    await expect(page.getByTestId("store-refused")).toHaveCount(0);

    await restorePhrase(page, MAIN_PHRASE);
    await page.getByTestId("onboarding-passphrase-new").fill("restored pass");
    await page.getByTestId("onboarding-passphrase-repeat").fill("restored pass");
    await page.getByTestId("onboarding-passphrase-submit").click();
    await page.getByTestId("onboarding-open").click();
    await expect(page.getByTestId("hero")).toBeVisible();

    // A further wallet comes through Add a wallet, the phrase restore only there too.
    await command(page, { kind: "addWallet" });
    await expect(page.getByTestId("onboarding-welcome").locator("h1")).toHaveText("Add a wallet");
    await expectRestoreOnly(page);
    await expect(page.getByTestId("onboarding-welcome-text")).toHaveText(
      "Until Dash Wallet has finished its first sync, you can only add a wallet from its recovery phrase. The other ways to add a wallet come back after that.",
    );
    await restorePhrase(page, SECOND_PHRASE);
    await page.getByTestId("onboarding-open").click();
    await expect(page.getByTestId("toolbar-wallet-picker")).toBeVisible();
    expect(await page.evaluate(() => Object.values(window.__dwFixture!.world.phrases).sort())).toEqual([MAIN_PHRASE, SECOND_PHRASE].sort());
    expect(await page.evaluate(() => window.__dwFixture!.world.restored)).toBe(true);

    // Once the mark clears (here set directly), Add a wallet offers every way again.
    await page.evaluate(() => (window.__dwFixture!.world.restored = false));
    await relaunch(page);
    await page.getByTestId("lock-field").fill("restored pass");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("hero")).toBeVisible();
    await command(page, { kind: "addWallet" });
    for (const id of ["onboarding-create", "onboarding-restore", "onboarding-restore-file"]) await expect(page.getByTestId(id)).toBeVisible();
    await expect(page.getByTestId("onboarding-welcome-text")).toHaveText("Create a new wallet, or restore one with its recovery phrase or a backup file.");
    await page.getByTestId("onboarding-cancel").click();
    await page.evaluate(() => (window.__dwFixture!.world.restored = true));

    // Two more starts open the new folder: the wallets, never the refusal.
    for (let i = 0; i < 2; i++) {
      await relaunch(page);
      await page.getByTestId("lock-field").fill("restored pass");
      await page.keyboard.press("Enter");
      await expect(page.getByTestId("hero")).toBeVisible();
      await expect(page.getByTestId("store-refused")).toHaveCount(0);
      await expect(page.getByTestId("onboarding")).toHaveCount(0);
    }
    expect(await restoreCalls(page)).toEqual(["recover"]);
    expect(errors).toEqual([]);
  });
});
