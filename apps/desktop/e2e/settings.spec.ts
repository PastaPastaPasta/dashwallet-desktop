import { expect, test, type Page } from "@playwright/test";
import type { CommandName } from "../src/api/generated/commands";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_PASSPHRASE as PASS, FIXTURE_WALLET_ID, SECOND_WALLET_ID } from "../src/fixture/scenarios";
import { FIXTURE_BACKUP_DIR, FIXTURE_FILES } from "../src/fixture/settings";
import { SecurityText } from "../src/settings/security";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { SETTINGS_VIEWS, showSettingsView } from "./settings";
import { PAYEE } from "./wallet";

// U-04: Settings ▸ General, Security, Wallets, Backups and About on the fixture backend
// (src/fixture/settings.ts and the vault handlers). The view models' rules are tested by parity
// ID in src/settings/*.test.ts; here the screens drive them.

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
const world = <T,>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

/** Holds the next call of `cmd` until the returned function is called (a late reply). */
async function hold(page: Page, cmd: CommandName) {
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), cmd);
  return () => release.evaluate((r) => r());
}

const A = `settings-wallet-${FIXTURE_WALLET_ID.slice(0, 8)}`;
const B = `settings-wallet-${SECOND_WALLET_ID.slice(0, 8)}`;

/** Opens `scenario` on Settings ▸ `tab`; `theme` null leaves the appearance to the settings. */
async function settings(page: Page, tab: string, scenario = "funded", theme: Theme | null = "light") {
  const errors = trackPageErrors(page);
  await page.goto(theme ? fixtureUrl(scenario, theme) : `/?scenario=${scenario}`);
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId(`settings-tabs-${tab}`).click();
  return errors;
}

test.describe("General", () => {
  test("QT139_QT020_theUnitAndDigitsApplyEverywhereAtOnce", async ({ page }) => {
    const errors = await settings(page, "general");
    await page.getByTestId("settings-unit-milliDash").click();
    await expect(page.getByTestId("settings-unit-milliDash")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("settings-digits")).toHaveCount(0);
    expect(await world(page, (w) => w.settings.ui)).toMatchObject({ unit: "milliDash" });
    await expect(page.getByTestId("status-bar")).toContainText("mtDASH");
    await page.getByTestId("sidebar-home").click();
    // The exact amount: every decimal of mDASH (five), trailing zeros trimmed to two.
    await expect(page.getByTestId("hero-amount")).toContainText(/^\d[\d ]*\.\d{2,5}\s?mtDASH$/);
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByText("Amount (mtDASH)")).toBeVisible();
    await page.getByTestId("pay-amount").fill("0.123456");
    await expect(page.getByText("Enter an amount in mDASH, for example 0.25.")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("QT137_subtractFeeByDefaultStartsEachPayForm", async ({ page }) => {
    await settings(page, "general");
    await page.getByTestId("settings-subtract-fee").check();
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByTestId("pay-subtract-fee")).toBeChecked();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.1");
    await page.getByTestId("pay-subtract-fee").uncheck();
    await expect(page.getByTestId("pay-subtract-fee")).not.toBeChecked();
  });

  test("QT135_theAppearanceFollowsTheSettingWithoutALaunchTheme", async ({ page }) => {
    await settings(page, "general", "funded", null);
    await page.getByTestId("settings-theme-dark").click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await page.getByTestId("settings-theme-system").click();
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", /./);
    // A launch theme wins and says so.
    await settings(page, "general", "funded", "light");
    await expect(page.getByTestId("settings-display")).toContainText("Light is in use for this session. Your choice below applies the next time Dash Wallet starts normally.");
  });

  test("QT141_resetOptionsAsksFirst", async ({ page }) => {
    await settings(page, "general");
    await page.getByTestId("settings-unit-duffs").click();
    await page.getByTestId("settings-reset").click();
    await expect(page.getByTestId("settings-reset-sheet")).toContainText("Confirm options reset");
    await page.getByTestId("settings-reset-confirm").click();
    await expect(page.getByTestId("settings-reset-done")).toBeVisible();
    await expect(page.getByTestId("settings-unit-dash")).toHaveAttribute("aria-pressed", "true");
  });

  test("QT075_dustProtectionKeepsItsThresholdInRange", async ({ page }) => {
    await settings(page, "general");
    await expect(page.getByTestId("settings-dust-on")).toBeChecked();
    await page.getByTestId("settings-dust-threshold").fill("0");
    await page.getByTestId("settings-dust-save").click();
    await expect(page.getByTestId("settings-dust-problem")).toHaveText("The dust threshold must be between 1 and 1,000,000 duffs.");
    await page.getByTestId("settings-dust-threshold").fill("5000");
    await page.getByTestId("settings-dust-save").click();
    await expect.poll(() => world(page, (w) => w.settings.dust)).toBe(5000);
    await page.getByTestId("settings-dust-on").uncheck();
    await expect.poll(() => world(page, (w) => w.settings.dust)).toBeNull();
    await expect(page.getByTestId("settings-dust-threshold")).toBeDisabled();
  });

  test("QT075_theSwitchMovesBeforeTheEngineAnswers", async ({ page }) => {
    await settings(page, "general");
    const release = await hold(page, "set_dust_protection");
    // D36: the switch shows the choice while it is saved, not the engine's last answer.
    await page.getByTestId("settings-dust-on").uncheck();
    await expect(page.getByTestId("settings-dust-on")).not.toBeChecked();
    await expect(page.getByTestId("settings-dust-on")).toBeDisabled();
    await release();
    await expect.poll(() => world(page, (w) => w.settings.dust)).toBeNull();
    await expect(page.getByTestId("settings-dust-on")).not.toBeChecked();
  });

  test("rows without an engine say who brings them (D6, D7)", async ({ page }) => {
    await settings(page, "general");
    await expect(page.getByTestId("settings-network-choice")).toBeVisible();
    // MP-01 brought the local currency: a 0.1 install has not seen the disclosure, so it is off.
    await expect(page.getByTestId("settings-currency-on")).not.toBeChecked();
    await expect(page.getByTestId("settings-currency")).toContainText("rates.ctx.com");
    await expect(page.getByTestId("settings-proxy")).toContainText("Tor and SOCKS5 proxy support are coming soon.");
    await expect(page.getByTestId("settings-data-dir")).toContainText("/home/user/.local/share/dashwallet");
  });
});

test.describe("Security", () => {
  test("QT111_changePassphrase", async ({ page }) => {
    await settings(page, "security");
    await page.getByTestId("settings-change-passphrase").click();
    await page.getByTestId("settings-change-current").fill("wrong");
    await page.getByTestId("settings-change-new").fill("next");
    await page.getByTestId("settings-change-repeat").fill("next");
    await page.getByTestId("settings-change-submit").click();
    await expect(page.getByTestId("settings-change-error")).toContainText("attempts");
    await page.getByTestId("settings-change-current").fill(PASS);
    await page.getByTestId("settings-change-new").fill("next");
    await page.getByTestId("settings-change-repeat").fill("next");
    await page.getByTestId("settings-change-submit").click();
    await expect(page.getByTestId("settings-change-sheet")).toHaveCount(0);
    await expect(page.getByTestId("settings-security-notice")).toBeVisible();
    expect(await world(page, (w) => w.passphrase)).toBe("next");
  });

  test("QT111_encryptWalletLocksIt", async ({ page }) => {
    await settings(page, "security", "unencrypted");
    await expect(page.getByTestId("settings-change-passphrase")).toHaveCount(0);
    await page.getByTestId("settings-encrypt").click();
    await page.getByTestId("settings-encrypt-new").fill("fresh-pass");
    await page.getByTestId("settings-encrypt-repeat").fill("fresh-pass");
    await page.getByTestId("settings-encrypt-submit").click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    expect(await world(page, (w) => [w.lock.state, w.passphrase])).toEqual(["locked", "fresh-pass"]);
  });

  test("IOS016_paymentAuthenticationAsksForThePassphrase", async ({ page }) => {
    await settings(page, "security");
    await expect(page.getByTestId("settings-payment-auth")).toBeChecked();
    await page.getByTestId("settings-payment-auth").click();
    await page.getByTestId("settings-payment-auth-passphrase-field").fill(PASS);
    await page.getByTestId("settings-payment-auth-passphrase-submit").click();
    await expect(page.getByTestId("settings-payment-auth-sheet")).toHaveCount(0);
    await expect(page.getByTestId("settings-payment-auth")).not.toBeChecked();
  });

  test("IOS016_theQuestionCannotCloseWhileThePassphraseIsChecked", async ({ page }) => {
    await settings(page, "security");
    await page.getByTestId("settings-payment-auth").click();
    const release = await hold(page, "authorize");
    await page.getByTestId("settings-payment-auth-passphrase-field").fill(PASS);
    await page.getByTestId("settings-payment-auth-passphrase-submit").click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("authorize"))).toBe(true);
    // D31: it cannot close; the change it started is seen through.
    await expect(page.getByTestId("settings-payment-auth-sheet").getByRole("button", { name: "Close" })).toBeDisabled();
    await release();
    await expect(page.getByTestId("settings-payment-auth-sheet")).toHaveCount(0);
    await expect(page.getByTestId("settings-payment-auth")).not.toBeChecked();
  });

  test("IOS015_autoLockLocksAfterTheChosenIdleTime", async ({ page }) => {
    await page.clock.install();
    await settings(page, "security");
    await expect(page.getByTestId("settings-auto-lock")).toHaveValue("null");
    await page.getByTestId("settings-auto-lock").selectOption("60");
    await expect.poll(() => world(page, (w) => w.settings.ui.autoLockSecs)).toBe(60);
    await page.clock.runFor(61_000);
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    await expect(page.getByText("Locked automatically.")).toBeVisible();
  });

  test("IOS020_autohideBalanceIsSavedForTheNextStart", async ({ page }) => {
    await settings(page, "security");
    await page.getByTestId("settings-autohide").check();
    await expect.poll(() => world(page, (w) => w.settings.ui.autohideBalance)).toBe(true);
    // D14: it takes effect when the app starts (options.test.ts themeAndLanguage), not at once.
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("hero-amount")).not.toContainText("#");
  });

  test("IOS020_theSwitchMovesBeforeTheSettingIsSaved", async ({ page }) => {
    await settings(page, "security");
    const release = await hold(page, "set_ui_settings");
    // D36: a saved display setting shows the choice while dw-app writes it.
    await page.getByTestId("settings-autohide").check();
    await expect(page.getByTestId("settings-autohide")).toBeChecked();
    await release();
    await expect.poll(() => world(page, (w) => w.settings.ui.autohideBalance)).toBe(true);
    await expect(page.getByTestId("settings-autohide")).toBeChecked();
  });

  test("IOS009_deleteAllWithTypedSentence", async ({ page }) => {
    const errors = await settings(page, "security", "two-wallets");
    await page.getByTestId("settings-wipe").click();
    await page.getByTestId("settings-wipe-sentence").fill("I accept");
    await page.getByTestId("settings-wipe-continue").click();
    await expect(page.getByTestId("settings-wipe-error")).toBeVisible();
    await page.getByTestId("settings-wipe-sentence").fill(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-continue").click();
    await page.getByTestId("settings-wipe-passphrase-field").fill(PASS);
    await page.getByTestId("settings-wipe-passphrase-submit").click();
    // No wallet left: the sheet closes, onboarding opens and a toast says why.
    await expect(page.getByTestId("onboarding")).toBeVisible();
    await expect(page.getByText(SecurityText.wiped)).toBeVisible();
    await expect(page.getByTestId("settings-wipe-sheet")).toHaveCount(0);
    expect(await world(page, (w) => [w.wallets.length, w.lock.state])).toEqual([0, "noVault"]);
    expect(errors).toEqual([]);
  });

  test("IOS009_aVaultThatCannotBeDeletedIsSaidNotCoveredByOnboarding", async ({ page }) => {
    await settings(page, "security");
    await page.evaluate(() => window.__dwFixture!.fail("destroy_vault", "call"));
    await page.getByTestId("settings-wipe").click();
    await page.getByTestId("settings-wipe-sentence").fill(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-continue").click();
    await page.getByTestId("settings-wipe-passphrase-field").fill(PASS);
    await page.getByTestId("settings-wipe-passphrase-submit").click();
    // D33: the wallet list is empty, but onboarding waits while the sheet says what failed.
    await expect(page.getByTestId("settings-wipe-error")).toContainText("the vault could not be fully deleted");
    await expect(page.getByTestId("onboarding")).toHaveCount(0);
    // D39: Enter tries again; D38: a menu shortcut leaves the sheet where it is.
    await expect(page.getByTestId("settings-wipe-retry")).toBeFocused();
    await page.keyboard.press("Control+2");
    await expect(page.getByTestId("settings-wipe-sheet")).toBeVisible();
    await expect(page.getByTestId("settings-tabs-security")).toBeVisible();
    expect(await world(page, (w) => [w.wallets.length, w.lock.state === "noVault"])).toEqual([0, false]);
    await page.evaluate(() => window.__dwFixture!.fail("destroy_vault", null));
    await page.getByTestId("settings-wipe-retry").click();
    await page.getByTestId("settings-wipe-passphrase-field").fill(PASS);
    await page.getByTestId("settings-wipe-passphrase-submit").click();
    await expect(page.getByTestId("onboarding")).toBeVisible();
    expect(await world(page, (w) => w.lock.state)).toBe("noVault");
  });

  test("IOS009_IOS110_destructiveConfirmationsTakeNoFocus", async ({ page }) => {
    await settings(page, "security", "unencrypted");
    await page.getByTestId("settings-wipe").click();
    await page.getByTestId("settings-wipe-sentence").fill(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-continue").click();
    // D35: Enter right after the step opens does not delete everything.
    await expect(page.getByTestId("settings-wipe-confirm")).toBeVisible();
    await expect(page.getByTestId("settings-wipe-confirm")).not.toBeFocused();
    await page.keyboard.press("Escape");
    await page.getByTestId("settings-tabs-wallets").click();
    await page.getByTestId(`${A}-remove`).click();
    await expect(page.getByTestId("settings-remove-confirm")).toBeVisible();
    await expect(page.getByTestId("settings-remove-confirm")).not.toBeFocused();
  });

  test("IOS109_aWipeReadingItsListCannotClose", async ({ page }) => {
    await settings(page, "security", "unencrypted");
    await page.getByTestId("settings-wipe").click();
    await page.getByTestId("settings-wipe-sentence").fill(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-continue").click();
    // D41: from the first engine call the sheet cannot close and the button is off.
    const release = await hold(page, "wallet_load_states");
    await page.getByTestId("settings-wipe-confirm").click();
    const sheet = page.getByTestId("settings-wipe-sheet");
    await expect(page.getByTestId("settings-wipe-confirm")).toBeDisabled();
    await expect(sheet.getByRole("button", { name: "Close" })).toBeDisabled();
    await release();
    await expect(page.getByTestId("onboarding")).toBeVisible();
    expect(await world(page, (w) => w.lock.state)).toBe("noVault");
  });

  test("quick unlock is not yet, and says who brings it (D7)", async ({ page }) => {
    await settings(page, "security");
    await expect(page.getByTestId("settings-quick-unlock")).toContainText("Not available on this computer");
  });
});

test.describe("Wallets", () => {
  test("QT101_closeKeepsDataAndOpenBringsItBack", async ({ page }) => {
    await settings(page, "wallets", "two-wallets");
    await page.getByTestId(`${B}-close`).click();
    await expect(page.getByTestId(`${B}-state`)).toHaveText("Closed");
    await expect(page.getByTestId(`${B}-startup`)).not.toBeChecked();
    // The last open wallet stays open.
    await expect(page.getByTestId(`${A}-close`)).toBeDisabled();
    await expect(page.getByTestId(`${A}-remove`)).toBeDisabled(); // D26
    await page.getByTestId(`${B}-open`).click();
    await expect(page.getByTestId(`${A}-remove`)).toBeEnabled();
    await expect(page.getByTestId(`${B}-state`)).toHaveText("Open");
  });

  test("IOS110_renameAndRemove", async ({ page }) => {
    await settings(page, "wallets", "two-wallets");
    await page.getByTestId(`${B}-rename`).click();
    await page.getByTestId("settings-rename-field").fill("  ");
    await page.getByTestId("settings-rename-save").click();
    await expect(page.getByTestId("settings-rename-error")).toBeVisible();
    await page.getByTestId("settings-rename-field").fill("Rainy day");
    await page.getByTestId("settings-rename-save").click();
    await expect(page.getByTestId(B)).toContainText("Rainy day");
    await page.getByTestId(`${B}-remove`).click();
    await page.getByTestId("settings-remove-passphrase-field").fill("wrong");
    await page.getByTestId("settings-remove-passphrase-submit").click();
    await expect(page.getByTestId("settings-remove-passphrase-error")).toBeVisible();
    await page.getByTestId("settings-remove-passphrase-field").fill(PASS);
    await page.getByTestId("settings-remove-passphrase-submit").click();
    await expect(page.getByTestId("settings-wallets-notice")).toContainText("Rainy day");
    await expect(page.getByTestId(B)).toHaveCount(0);
  });

  test("IOS110_removeCannotCloseWhileThePassphraseIsChecked", async ({ page }) => {
    await settings(page, "wallets", "two-wallets");
    await page.getByTestId(`${B}-remove`).click();
    const release = await hold(page, "authorize");
    await page.getByTestId("settings-remove-passphrase-field").fill(PASS);
    await page.getByTestId("settings-remove-passphrase-submit").click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("authorize"))).toBe(true);
    // D31: the sheet cannot close until the removal it started is done, and says so.
    await expect(page.getByTestId("settings-remove-sheet").getByRole("button", { name: "Close" })).toBeDisabled();
    await release();
    await expect(page.getByTestId("settings-wallets-notice")).toBeVisible();
    await expect(page.getByTestId(B)).toHaveCount(0);
  });

  test("the Remove wallet warning matches the wallet's recovery material", async ({ page }) => {
    for (const [summary, warning] of [
      [{ hasMnemonic: true, legacyIssued: false }, "recovery phrase and its BIP39 passphrase"],
      [{ hasMnemonic: false, legacyIssued: false }, "Only a wallet backup or export"],
      [{ hasMnemonic: true, legacyIssued: true }, "Only a wallet backup or export"],
    ] as const) {
      await page.goto(fixtureUrl("two-wallets", "light"));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate((state) => Object.assign(window.__dwFixture!.world.wallets[1].summary, state), summary);
      await command(page, { kind: "navigate", route: "settings" });
      await page.getByTestId("settings-tabs-wallets").click();
      await page.getByTestId(`${B}-remove`).click();
      await expect(page.getByTestId("settings-remove-sheet")).toContainText(warning);
    }
  });

  for (const [reason, text] of [
    ["inFlight", "A Shielded payment from this wallet may still be going through, so nothing was removed. Try again once it has confirmed."],
    [
      "carriedOver",
      "Dash Wallet can't tell whether an earlier Shielded payment from this wallet went through, so nothing was removed. You can keep using this wallet, but this version may not be able to remove it.",
    ],
  ] as const) {
    test(`a removal refused (${reason}) says why and removes nothing`, async ({ page }) => {
      await settings(page, "wallets", "two-wallets");
      await page.evaluate(`window.__dwFixture.world.settings.removeRefusal = ${JSON.stringify(reason)}`);
      await page.getByTestId(`${B}-remove`).click();
      await page.getByTestId("settings-remove-passphrase-field").fill(PASS);
      await page.getByTestId("settings-remove-passphrase-submit").click();
      await expect(page.getByTestId("settings-remove-sheet")).toContainText("This wallet can't be removed yet");
      await expect(page.getByTestId("settings-remove-held")).toHaveText(text);
      await page.getByTestId("settings-remove-held-ok").click();
      await expect(page.getByTestId("settings-remove-sheet")).toHaveCount(0);
      await expect(page.getByTestId(B)).toBeVisible();
      await expect(page.getByTestId("settings-wallets-notice")).toHaveCount(0);
      expect(await world(page, (w) => w.wallets.length)).toBe(2);
    });
  }

  test("settings_removal_of_a_wallet_whose_held_coin_now_awaits_shows_the_waiting_text_before_confirming", async ({ page }) => {
    await settings(page, "wallets", "two-wallets");
    await page.getByTestId(`${B}-remove`).click();
    await expect(page.getByTestId("settings-remove-sheet")).toBeVisible();
    // heldAwaiting becomes true while the user is already in Settings.
    await page.evaluate(() => {
      const w = window.__dwFixture!.world.wallets[1].summary;
      w.balances = { ...w.balances!, heldUnfinished: 50_000_000, heldAwaiting: true };
    });
    await page.getByTestId("settings-remove-passphrase-field").fill(PASS);
    await page.getByTestId("settings-remove-passphrase-submit").click();
    await expect(page.getByTestId("settings-remove-sheet")).toContainText("Wait before removing this wallet");
    await expect(page.getByTestId("settings-remove-awaiting")).toContainText("isn't final on the network yet");
    await expect(page.getByTestId("settings-remove-anyway")).toHaveText("Remove anyway");
    // Cancel leaves the wallet in place.
    await page.getByTestId("settings-remove-awaiting-cancel").click();
    await expect(page.getByTestId("settings-remove-sheet")).toHaveCount(0);
    await expect(page.getByTestId(B)).toBeVisible();
    expect(await world(page, (w) => w.wallets.length)).toBe(2);

    // Confirming "Remove anyway" removes the wallet.
    await page.getByTestId(`${B}-remove`).click();
    await page.getByTestId("settings-remove-passphrase-field").fill(PASS);
    await page.getByTestId("settings-remove-passphrase-submit").click();
    await expect(page.getByTestId("settings-remove-awaiting")).toBeVisible();
    await page.getByTestId("settings-remove-anyway").click();
    await expect(page.getByTestId("settings-wallets-notice")).toContainText("Savings");
    await expect(page.getByTestId(B)).toHaveCount(0);
  });

  test("IOS111_xpubWithQRCode_and_QT114_watchOnlyFromIt", async ({ page }) => {
    await settings(page, "wallets");
    await page.getByTestId(`${A}-xpub`).click();
    await expect(page.getByTestId("settings-xpub-qr")).toBeVisible();
    await expect(page.getByTestId("settings-xpub-path")).toContainText("m/44'/1'/0'");
    const xpub = (await page.getByTestId("settings-xpub-key").locator(".mono").innerText()).trim();
    expect(xpub).toMatch(/^tpub/);
    await page.keyboard.press("Escape");
    await page.getByTestId("settings-add-watch-only").click();
    await page.getByTestId("settings-watch-only-xpub").fill("xpub-garbage");
    await page.getByTestId("settings-watch-only-add").click();
    await expect(page.getByTestId("settings-watch-only-error")).toBeVisible();
    await page.getByTestId("settings-watch-only-xpub").fill(xpub);
    await page.getByTestId("settings-watch-only-name").fill("Cold storage");
    await page.getByTestId("settings-watch-only-add").click();
    await expect(page.getByTestId("settings-wallet-list")).toContainText("Cold storage");
    await expect(page.getByTestId("settings-wallet-list")).toContainText("Watch-only");
  });
});

test.describe("Backups", () => {
  test("QT110_backUpWritesANamedFile", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-backup").click();
    await page.getByTestId("settings-backup-write").click();
    await expect(page.getByTestId("settings-backup-path")).toContainText(`${FIXTURE_BACKUP_DIR}/Main-`);
    expect(Object.keys(await world(page, (w) => w.settings.files)).some((p) => p.endsWith(".dwbackup") && !p.includes("-auto-"))).toBe(true);
  });

  test("QT110_anUnencryptedVaultsBackupAsksForAPassphrase", async ({ page }) => {
    await settings(page, "backups", "unencrypted");
    await page.getByTestId("settings-backup").click();
    await page.getByTestId("settings-backup-passphrase-new").fill("backup-pass");
    await page.getByTestId("settings-backup-passphrase-repeat").fill("backup-pass");
    await page.getByTestId("settings-backup-passphrase-submit").click();
    await expect(page.getByTestId("settings-backup-done")).toBeVisible();
  });

  test("QT116_automaticBackupsListAndKeep", async ({ page }) => {
    await settings(page, "backups");
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    await expect(page.getByTestId("settings-backup-folder")).toContainText(FIXTURE_BACKUP_DIR);
    const keep = await world(page, (w) => w.settings.keep);
    // D30: a count that deletes files asks first, and says how many and where.
    await page.getByTestId("settings-backup-keep").selectOption("0");
    await expect(page.getByTestId("settings-backup-keep-sheet")).toContainText(`deletes every automatic backup in ${FIXTURE_BACKUP_DIR} now`);
    await expect(page.getByTestId("settings-backup-keep-sheet")).toContainText("The list shown counts 2 of them; there can be more.");
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByTestId("settings-backup-keep-sheet")).toHaveCount(0);
    expect(await world(page, (w) => w.settings.keep)).toBe(keep);
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    await expect(page.getByTestId("settings-backup-keep")).toHaveValue(String(keep));
    await page.getByTestId("settings-backup-keep").selectOption("0");
    await page.getByTestId("settings-backup-keep-confirm").click();
    await expect.poll(() => world(page, (w) => w.settings.keep)).toBe(0);
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(0);
  });

  // Issue 340: the engine's failed automatic backup shows as a toast at once and stays in the card,
  // with what to do, until backups are turned off (or one succeeds, which the engine decides).
  test("QT116_aFailedAutomaticBackupSaysWhatToDo", async ({ page }) => {
    await settings(page, "backups");
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    await expect(page.getByTestId("settings-backup-failed")).toHaveCount(0);
    const failedAt = await world(page, (w) => w.settings.backups[0].createdAt + 60);
    await page.evaluate((failedAt) => {
      const f = window.__dwFixture!;
      f.world.settings.backupFailures.push({ walletId: f.world.wallets[0].summary.walletId, failedAt });
      f.emit({ kind: "notice", network: "testnet", code: "backupFailed", detail: "wallet: cannot write" });
    }, failedAt);
    await expect(page.locator(".toast")).toHaveText("An automatic backup failed. Open Settings ▸ Backups to see what to do.");
    const row = page.getByTestId("settings-backup-failed");
    await expect(row).toHaveCount(1);
    await expect(row).toContainText("The latest automatic backup failed");
    const year = new Date(failedAt * 1000).getUTCFullYear();
    await expect(row).toContainText(new RegExp(`Dash Wallet could not write an automatic backup of Main on [^.]*${year}`));
    await expect(row).toContainText("and that Dash Wallet can write to it.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await row.getByTestId("settings-backup-failed-back-up").click();
    await expect(page.getByTestId("settings-backup-sheet")).toContainText("Back up Main");
    await page.getByRole("button", { name: "Close" }).click();
    // A reload reads it again from the engine; turning automatic backups off clears it.
    await page.getByTestId("settings-tabs-general").click();
    await page.getByTestId("settings-tabs-backups").click();
    await expect(row).toHaveCount(1);
    await page.getByTestId("settings-backup-keep").selectOption("0");
    await page.getByTestId("settings-backup-keep-confirm").click();
    await expect(row).toHaveCount(0);
  });

  test("QT116_aBackupWrittenAfterTheListWasReadIsNotDeletedWithoutAsking", async ({ page }) => {
    await settings(page, "backups");
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    // The engine writes a third backup in the background and says nothing (D28): the list shown
    // keeps two.
    await page.evaluate(() => {
      const s = window.__dwFixture!.world.settings;
      const [b] = s.backups;
      s.backups.push({ ...b, path: `${b.path}.new`, createdAt: b.createdAt + 1 });
    });
    const listed = await page.getByTestId("settings-backup-list").locator("li").allTextContents();
    const oldest = await world(page, (w) => w.settings.backups.toSorted((a, b) => a.createdAt - b.createdAt)[0].path);
    // D42: by the list keeping 2 deletes nothing, but the engine deletes the oldest of three.
    await page.getByTestId("settings-backup-keep").selectOption("2");
    const sheet = page.getByTestId("settings-backup-keep-sheet");
    await expect(sheet).toContainText("each wallet's newest 2");
    await expect(sheet).toContainText("so backups shown here can go too. The list shown counts none of them; there can be more.");
    await page.getByRole("button", { name: "Cancel" }).click();
    expect(await world(page, (w) => [w.settings.keep, w.settings.backups.length])).toEqual([5, 3]);
    // D46: confirmed, the prune takes a backup the list shows, as the question warned.
    expect(listed.some((t) => t.includes(oldest))).toBe(true);
    await page.getByTestId("settings-backup-keep").selectOption("2");
    await page.getByTestId("settings-backup-keep-confirm").click();
    await expect.poll(() => world(page, (w) => w.settings.keep)).toBe(2);
    expect(await world(page, (w) => w.settings.backups.map((b) => b.path))).not.toContain(oldest);
    expect(await world(page, (w) => w.settings.backups.some((b) => b.path.endsWith(".new")))).toBe(true);
  });

  test("QT116_aListStillBeingReadAsksBeforeALowerCount", async ({ page }) => {
    await settings(page, "backups");
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    // Four more backups appear while another tab shows; the list shown next is the old one until
    // it is read again (D28), and that read is held.
    await page.evaluate(() => {
      const s = window.__dwFixture!.world.settings;
      const [b] = s.backups;
      for (let i = 1; i <= 4; i++) s.backups.push({ ...b, path: `${b.path}.${i}`, createdAt: b.createdAt + i });
    });
    await page.getByTestId("settings-tabs-general").click();
    const release = await hold(page, "automatic_backups");
    await page.getByTestId("settings-tabs-backups").click();
    await expect(page.getByTestId("settings-backup-list").locator("li")).toHaveCount(2);
    // D30: by the old list keeping 3 deletes nothing; a list still being read is not counted, so
    // the question gives no number (every lower count asks, D42).
    await page.getByTestId("settings-backup-keep").selectOption("3");
    await expect(page.getByTestId("settings-backup-keep-sheet")).toContainText("each wallet's newest 3 automatic backups");
    await expect(page.getByTestId("settings-backup-keep-sheet")).not.toContainText("The list shown counts");
    expect(await world(page, (w) => [w.settings.keep, w.settings.backups.length])).toEqual([5, 6]);
    await release();
  });

  test("QT109_exportForCoreNeedsThePassphrase", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-export").click();
    await page.getByTestId("settings-export-descriptors").click();
    await page.getByTestId("settings-export-passphrase-field").fill(PASS);
    await page.getByTestId("settings-export-passphrase-submit").click();
    await expect(page.getByTestId("settings-export-done")).toContainText("62");
    await expect(page.getByTestId("settings-export-path")).toContainText("-descriptors.json");
  });

  test("Show Folder opens the backup folder in the file manager, and says why when it can't", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-show-folder").click();
    await expect.poll(() => world(page, (w) => w.settings.foldersOpened ?? 0)).toBe(1);
    await expect(page.getByTestId("settings-show-folder-error")).toHaveCount(0);
    await page.evaluate(() => (window.__dwFixture!.world.settings.folderOpenFails = true));
    await page.getByTestId("settings-show-folder").click();
    await expect(page.getByTestId("settings-show-folder-error")).toContainText("The backup folder couldn't be opened.");
    await page.evaluate(() => (window.__dwFixture!.world.settings.backupFolderMissing = true));
    await page.getByTestId("settings-show-folder").click();
    await expect(page.getByTestId("settings-show-folder-error")).toContainText("There's no backup folder yet.");
    // Once it opens again, the message goes.
    await page.evaluate(() => {
      const s = window.__dwFixture!.world.settings;
      s.backupFolderMissing = false;
      s.folderOpenFails = false;
    });
    await page.getByTestId("settings-show-folder").click();
    await expect.poll(() => world(page, (w) => w.settings.foldersOpened ?? 0)).toBe(2);
    await expect(page.getByTestId("settings-show-folder-error")).toHaveCount(0);
  });

  test("Choose File… fills the import path from the system's open dialog; a cancel keeps it", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-import-file").click();
    await page.getByTestId("settings-import-path").fill("/home/user/typed.dat");
    await page.getByTestId("settings-import-browse").click();
    await expect(page.getByTestId("settings-import-path")).toHaveValue("/home/user/typed.dat");
    // A dialog that fails says so; the next pick clears it.
    await page.evaluate(() => (window.__dwFixture!.world.tools.files.dialogFails = true));
    await page.getByTestId("settings-import-browse").click();
    await expect(page.getByTestId("settings-import-error")).toBeVisible();
    await page.evaluate(() => (window.__dwFixture!.world.tools.files.open = { name: "dash-core-dumpwallet.txt", data: "" }));
    await page.getByTestId("settings-import-browse").click();
    await expect(page.getByTestId("settings-import-path")).toHaveValue("/home/user/Documents/dash-core-dumpwallet.txt");
    await expect(page.getByTestId("settings-import-error")).toHaveCount(0);
    const dialogs = await page.evaluate(() => window.__dwFixture!.world.tools.files.dialogs.map((d) => [d.purpose, d.save]));
    expect(dialogs).toEqual([["walletFile", false], ["walletFile", false], ["walletFile", false]]);
  });

  test("a legacy wallet.dat refused on purpose says why and what to do instead", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-import-file").click();
    await page.getByTestId("settings-import-path").fill(FIXTURE_FILES.walletDatTooLarge);
    await page.getByTestId("settings-import-inspect").click();
    await expect(page.getByTestId("settings-import-error")).toHaveText(/more than 10,000 addresses in a row.*send its funds to an address from Receive in Dash Wallet/);
    await page.getByTestId("settings-import-path").fill(FIXTURE_FILES.walletDatTwoAccounts);
    await page.getByTestId("settings-import-inspect").click();
    // A file whose only account is not the first: the advice names no destination inside the file.
    await expect(page.getByTestId("settings-import-error")).toHaveText(/an HD account other than the first one.*an address from Receive in Dash Wallet/);
    await expect(page.getByTestId("settings-import-error")).not.toContainText("same wallet");
    await expect(page.getByTestId("settings-import-error")).not.toContainText("cannot read this kind of file");
  });

  test("QT107_importADumpwalletFile", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-import-file").click();
    await page.getByTestId("settings-import-path").fill(FIXTURE_FILES.mainnetDump);
    await page.getByTestId("settings-import-inspect").click();
    await expect(page.getByTestId("settings-import-refusal")).toContainText("mainnet");
    await page.getByTestId("settings-import-back").click();
    await page.getByTestId("settings-import-path").fill(FIXTURE_FILES.dumpwallet);
    await page.getByTestId("settings-import-inspect").click();
    await page.getByTestId("settings-import-name").fill("From Core");
    await page.getByTestId("settings-import-run").click();
    await expect(page.getByTestId("settings-import-done-0")).toContainText("3");
    await expect(page.getByTestId("settings-import-done-1")).toContainText("2");
    await page.getByTestId("settings-import-close").click();
    await page.getByTestId("settings-tabs-wallets").click();
    await expect(page.getByTestId("settings-wallet-list")).toContainText("From Core");
  });

  test("QT108_importKeysChecksTheirShape", async ({ page }) => {
    await settings(page, "backups");
    await page.getByTestId("settings-import-keys").click();
    await page.getByTestId("settings-keys-hdSeed").click();
    await page.getByTestId("settings-keys-text").fill("abc");
    await page.getByTestId("settings-keys-import").click();
    await expect(page.getByTestId("settings-keys-error")).toBeVisible();
    await page.getByTestId("settings-keys-text").fill("0f".repeat(32));
    await page.getByTestId("settings-keys-import").click();
    await expect(page.getByTestId("settings-import-done-0")).toBeVisible();
  });
});

test.describe("restore-only (RESTORED set)", () => {
  test("Settings offers no way to add a wallet but the phrase, and the others come back once the mark clears", async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("funded", "light", "&restored=yes"));
    await page.getByTestId("toolbar").waitFor();
    await command(page, { kind: "navigate", route: "settings" });
    await page.getByTestId("settings-tabs-wallets").click();
    await expect(page.getByTestId("settings-add-wallet")).toBeVisible();
    await expect(page.getByTestId("settings-wallets")).toBeVisible();
    await expect(page.getByTestId("settings-add-watch-only")).toHaveCount(0);
    await page.getByTestId("settings-tabs-backups").click();
    await expect(page.getByTestId("settings-automatic-backups")).toBeVisible();
    await expect(page.getByTestId("settings-import-card")).toHaveCount(0);
    await expect(page.getByTestId("settings-import-file")).toHaveCount(0);
    await expect(page.getByTestId("settings-import-keys")).toHaveCount(0);

    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.restored = false;
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("settings-import-file")).toBeVisible();
    await expect(page.getByTestId("settings-import-keys")).toBeVisible();
    await page.getByTestId("settings-tabs-wallets").click();
    await expect(page.getByTestId("settings-add-watch-only")).toBeVisible();
    expect(errors).toEqual([]);
  });
});

test.describe("About", () => {
  test("QT001_QT002_versionsNetworkAndFolders", async ({ page }) => {
    await settings(page, "about");
    for (const id of ["version", "engine", "network", "agent", "data-dir", "backups"]) await expect(page.getByTestId(`settings-about-${id}`)).not.toBeEmpty();
    await expect(page.getByTestId("settings-about-network").locator("dd")).toHaveText("testnet");
    await expect(page.getByTestId("settings-about-backups")).toContainText(FIXTURE_BACKUP_DIR);
  });

  test("project links open by name, and a failed open gives the link to copy", async ({ page }) => {
    await settings(page, "about");
    await expect(page.getByTestId("settings-link-issues-hint")).toHaveText(/^Reports are public\./);
    for (const link of ["issues", "sourceCode", "website"]) await page.getByTestId(`settings-link-${link}`).click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.linksOpened)).toEqual(["issues", "sourceCode", "website"]);
    await expect(page.getByTestId("settings-link-failed")).toHaveCount(0);

    await page.evaluate(() => (window.__dwFixture!.world.linkOpenFails = true));
    await page.getByTestId("settings-link-website").click();
    await expect(page.getByTestId("settings-link-failed")).toHaveText(/^The browser couldn't be opened\. Copy the link instead: https:\/\/www\.dash\.org/);
    await page.getByTestId("settings-link-issues").click();
    await expect(page.getByTestId("settings-link-failed")).toContainText("https://github.com/PastaPastaPasta/dashwallet-desktop/issues");
  });
});

test.describe("axe", () => {
  for (const theme of THEMES)
    for (const view of SETTINGS_VIEWS)
      test(`axe settings-${view} ${theme}`, async ({ page }) => {
        const errors = trackPageErrors(page);
        await showSettingsView(page, view, theme);
        await fontsReady(page);
        expect(await seriousAxeViolations(page)).toEqual([]);
        expect(errors).toEqual([]);
      });
});
