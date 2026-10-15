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
    await page.getByTestId("settings-digits").selectOption("4");
    expect(await world(page, (w) => w.settings.ui)).toMatchObject({ unit: "milliDash", decimalDigits: 4 });
    await expect(page.getByTestId("status-bar")).toContainText("mtDASH");
    await page.getByTestId("sidebar-home").click();
    // Floored to four decimals of mDASH (five decimals): one digit cut, never rounded.
    await expect(page.getByTestId("hero-amount")).toContainText(/^\d[\d ]*\.\d{4}\s?mtDASH$/);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await expect(page.getByText("Amount (mtDASH)")).toBeVisible();
    await page.getByTestId("pay-amount").fill("0.123456");
    await expect(page.getByText("Enter an amount in mDASH, for example 0.25.")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("QT137_subtractFeeByDefaultStartsEachPayForm", async ({ page }) => {
    await settings(page, "general");
    await page.getByTestId("settings-subtract-fee").check();
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
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
    await expect(page.getByTestId("settings-display")).toContainText("--theme light");
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
    await expect(page.getByTestId("settings-switch-network")).toContainText("U-04b");
    await expect(page.getByTestId("settings-currency")).toContainText("MP-01");
    await expect(page.getByTestId("settings-proxy")).toContainText("Not available yet");
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
    await expect(page.getByTestId("settings-backup-keep-sheet")).toContainText("Of the backups listed here, 2 go");
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
    // D42: by the list keeping 2 deletes nothing, but the engine would delete the oldest of three.
    await page.getByTestId("settings-backup-keep").selectOption("2");
    await expect(page.getByTestId("settings-backup-keep-sheet")).toContainText("None of the backups listed here goes, only any written since the list was read.");
    await page.getByRole("button", { name: "Cancel" }).click();
    expect(await world(page, (w) => [w.settings.keep, w.settings.backups.length])).toEqual([5, 3]);
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
    // D30: by the old list keeping 3 deletes nothing; it is unknown, so the count asks.
    await page.getByTestId("settings-backup-keep").selectOption("3");
    await expect(page.getByTestId("settings-backup-keep-sheet")).toBeVisible();
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

test.describe("About", () => {
  test("QT001_QT002_versionsNetworkAndFolders", async ({ page }) => {
    await settings(page, "about");
    for (const id of ["version", "engine", "network", "agent", "data-dir", "backups"]) await expect(page.getByTestId(`settings-about-${id}`)).not.toBeEmpty();
    await expect(page.getByTestId("settings-about-network").locator("dd")).toHaveText("testnet");
    await expect(page.getByTestId("settings-about-backups")).toContainText(FIXTURE_BACKUP_DIR);
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
