import { expect, test, type Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID, SECOND_WALLET_ID, type World } from "../src/fixture/scenarios";
import { CORE_ONLY_PHRASE, CREATED_12, CREATED_24, MAIN_PHRASE, SECOND_PHRASE } from "../src/fixture/vault";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// U-02 on the fixture backend: create with verify, restore with live checks, the vault
// passphrase, the lock screen's lockout, For mixing only and the unlock sheet, Forgot passphrase,
// Show Recovery Phrase and Add Wallet; every step with 0 serious axe issues in light and dark.

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.locator("[data-testid=onboarding], [data-testid=hero], [data-testid=lock-screen]").first().waitFor();
  return errors;
}

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
/** Reads the fixture's world in the page (`read` is sent as source, so it captures nothing). */
const world = <T>(page: Page, read: (w: World) => T) =>
  page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;
const calls = (page: Page) => page.evaluate(() => window.__dwFixture!.calls);
const secretShown = (page: Page) => page.evaluate(() => window.__dwFixture!.world.shellSync?.secretShown);

async function axe(page: Page) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
}

/** Picks the asked words of `phrase` in order on the verify step. */
async function verify(page: Page, phrase: string) {
  const words = phrase.split(" ");
  const labels = await page.getByTestId("verify-slot").locator(".verify-slot-label").allTextContents();
  for (const label of labels) {
    const position = Number(label.replace("Word ", "")) - 1;
    await page.getByTestId("verify-chip").filter({ hasText: new RegExp(`^${words[position]}$`) }).click();
  }
}

async function choosePassphrase(page: Page, passphrase: string, repeat = passphrase) {
  await page.getByTestId("onboarding-passphrase-new").fill(passphrase);
  await page.getByTestId("onboarding-passphrase-repeat").fill(repeat);
  await page.getByTestId("onboarding-passphrase-submit").click();
}

test("create: passphrase first, the phrase, verify with a wrong word, then the wallet (IOS-002/004/010)", async ({ page }) => {
  const errors = await open(page, "no-wallet");
  await expect(page.getByTestId("onboarding-welcome")).toBeVisible();
  // First run: nothing to cancel.
  await expect(page.getByTestId("onboarding-cancel")).toHaveCount(0);
  // The window is content-protected for the whole flow, before any phrase shows (D15, IOS-006).
  await expect.poll(() => secretShown(page)).toBe(true);
  await page.getByTestId("onboarding-create").click();
  await expect(page.getByTestId("onboarding-choosePassphrase")).toBeVisible();
  await expect(page.getByTestId("onboarding-passphrase-new")).toBeFocused();
  await page.getByTestId("onboarding-passphrase-new").fill("short");
  await expect(page.getByTestId("passphrase-strength")).toHaveAttribute("data-strength", "weak");
  await choosePassphrase(page, "Fixture pass 42!", "something else");
  await expect(page.getByTestId("onboarding-passphrase-error")).toHaveText("The supplied passphrases do not match.");
  // The fields are cleared after each attempt.
  await expect(page.getByTestId("onboarding-passphrase-new")).toHaveValue("");
  await choosePassphrase(page, "Fixture pass 42!");

  await expect(page.getByTestId("onboarding-showPhrase")).toBeVisible();
  await expect(page.getByTestId("onboarding-phrase-words").locator(".phrase-word")).toHaveText(CREATED_12.split(" "));
  // No copy (D18): the words cannot be selected.
  await expect(page.getByTestId("onboarding-phrase-words")).toHaveCSS("user-select", "none");
  await expect.poll(() => secretShown(page)).toBe(true);
  // Nothing is stored yet.
  expect(await calls(page)).not.toContain("create_vault");
  await expect(page.getByTestId("onboarding-phrase-continue")).toBeDisabled();
  await page.getByTestId("onboarding-written-down").check();
  await page.getByTestId("onboarding-phrase-continue").click();

  await expect(page.getByTestId("onboarding-verifyPhrase")).toBeVisible();
  await expect(page.getByTestId("verify-slot")).toHaveCount(4);
  await expect(page.getByTestId("verify-chip")).toHaveCount(6);
  // The phrase grid is gone: the verify step shows only its chips.
  await expect(page.getByTestId("onboarding-phrase-words")).toHaveCount(0);
  const labels = await page.getByTestId("verify-slot").locator(".verify-slot-label").allTextContents();
  const first = CREATED_12.split(" ")[Number(labels[0].replace("Word ", "")) - 1];
  await page.getByTestId("verify-chip").filter({ hasNotText: new RegExp(`^${first}$`) }).first().click();
  await expect(page.getByTestId("verify-mistake")).toHaveText("Wrong word. Try again.");
  await verify(page, CREATED_12);

  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  expect(await calls(page)).toEqual(expect.arrayContaining(["create_vault", "import_wallet"]));
  expect(await world(page, (w) => w.passphrase)).toBe("Fixture pass 42!");
  await page.getByTestId("onboarding-open").click();
  await expect(page.getByTestId("onboarding")).toHaveCount(0);
  await expect.poll(() => secretShown(page)).toBe(false);
  await expect(page.getByTestId("hero")).toBeVisible();
  await expect(page.getByTestId("status-bar")).toContainText("Unlocked");
  expect(errors).toEqual([]);
});

test("create: 24 words under Advanced options, and without a passphrase", async ({ page }) => {
  await open(page, "no-wallet");
  await page.locator(".onboarding-advanced summary").click();
  await page.getByTestId("onboarding-24-words").check();
  await page.getByTestId("onboarding-create").click();
  await page.locator(".onboarding-advanced summary").click();
  await page.getByTestId("onboarding-skip-passphrase").click();
  await expect(page.getByTestId("onboarding-phrase-words").locator(".phrase-word")).toHaveText(CREATED_24.split(" "));
  // Back from the phrase returns to Welcome and drops it.
  await page.getByTestId("onboarding-back").click();
  await expect(page.getByTestId("onboarding-welcome")).toBeVisible();
  expect(await calls(page)).not.toContain("import_wallet");
  await page.getByTestId("onboarding-create").click();
  await page.locator(".onboarding-advanced summary").click();
  await page.getByTestId("onboarding-skip-passphrase").click();
  await page.getByTestId("onboarding-written-down").check();
  await page.getByTestId("onboarding-phrase-continue").click();
  await verify(page, CREATED_24);
  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  expect(await world(page, (w) => w.lock.encrypted)).toBe(false);
});

test("restore: live checks, suggestions, Dash Core only, options, passphrase last (IOS-007, QT-104)", async ({ page }) => {
  await open(page, "no-wallet");
  await page.getByTestId("onboarding-restore").click();
  const input = page.getByTestId("restore-input");
  await expect(input).toBeFocused();
  await expect(input).toHaveAttribute("spellcheck", "false");
  await expect(input).toHaveAttribute("autocomplete", "off");
  // A word being typed is not wrong yet; an earlier unknown word is.
  await input.fill("enjoy congxx lay");
  await expect(page.getByTestId("restore-problem")).toHaveText("Word 2 is not in the word list.");
  await input.fill("enjoy congress lay");
  await expect(page.getByTestId("restore-problem")).toHaveCount(0);
  await expect(page.getByTestId("restore-suggestion").first()).toHaveText("layer");
  await page.getByTestId("restore-suggestion").first().click();
  await expect(input).toHaveValue("enjoy congress layer ");
  // Tab completes the first suggestion.
  await input.focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.type("cro");
  await page.keyboard.press("Tab");
  await expect(input).toHaveValue("enjoy congress layer crop ");
  await expect(page.getByTestId("restore-continue")).toBeDisabled();

  await input.fill(CORE_ONLY_PHRASE);
  await expect(page.getByTestId("restore-valid")).toContainText("Dash Core's legacy checksum");
  await input.fill(`  ${SECOND_PHRASE.toUpperCase()}  `);
  await expect(page.getByTestId("restore-valid")).toHaveText("The recovery phrase is valid.");
  await expect.poll(() => secretShown(page)).toBe(true);
  await page.getByTestId("restore-continue").click();

  await expect(page.getByTestId("onboarding-restoreOptions")).toBeVisible();
  await expect(page.getByTestId("restore-scan-start")).toHaveText("The scan starts at the first block.");
  await page.getByTestId("restore-bip39").fill("twenty-fifth");
  await page.getByTestId("restore-scan-height").check();
  await page.getByTestId("restore-height").fill("1200");
  await expect(page.getByTestId("restore-scan-start")).toHaveText("The scan starts at block 1,200.");
  // Clearing the height scans from genesis again, never from the tip.
  await page.getByTestId("restore-height").fill("");
  await expect(page.getByTestId("restore-scan-start")).toHaveText("The scan starts at the first block.");
  await page.getByTestId("restore-height").fill("1200");
  await page.getByTestId("restore-finish").click();

  // No vault yet: the passphrase comes last.
  await expect(page.getByTestId("onboarding-choosePassphrase")).toBeVisible();
  await choosePassphrase(page, "restored pass");
  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  await expect(page.getByTestId("restore-input")).toHaveCount(0);
  const wallets = await world(page, (w) => w.wallets.map((x) => x.summary.walletId));
  expect(wallets).toHaveLength(1);
  expect(await world(page, (w) => Object.values(w.phrases))).toEqual([SECOND_PHRASE]);
  await page.getByTestId("onboarding-open").click();
  await expect(page.getByTestId("hero")).toBeVisible();
});

test("restore into a vault that has no wallet yet: no passphrase step", async ({ page }) => {
  await open(page, "no-wallet-vault");
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(MAIN_PHRASE);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  expect(await calls(page)).not.toContain("create_vault");
});

test("Add Wallet: the menu and Settings open it over the shell, Cancel closes it, the lock drops it", async ({ page }) => {
  await open(page, "funded");
  await command(page, { kind: "addWallet" });
  await expect(page.getByTestId("onboarding-welcome")).toBeVisible();
  await expect(page.getByTestId("onboarding-welcome").locator("h1")).toHaveText("Add a wallet");
  await page.getByTestId("onboarding-cancel").click();
  await expect(page.getByTestId("onboarding")).toHaveCount(0);

  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-add-wallet").click();
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(SECOND_PHRASE);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await page.getByTestId("onboarding-open").click();
  await expect(page.getByTestId("toolbar-wallet-picker")).toBeVisible();
  // The window shows the wallet it added.
  const added = await world(page, (w) => w.wallets.at(-1)!.summary.walletId);
  await expect(page.getByTestId("toolbar-wallet-picker")).toHaveValue(added);

  // Asked for while locked, it is dropped.
  await command(page, { kind: "lock" });
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await command(page, { kind: "addWallet" });
  await expect(page.getByTestId("onboarding")).toHaveCount(0);
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await expect(page.getByTestId("onboarding")).toHaveCount(0);
});

test("Add Wallet while unlocked for mixing only asks for the passphrase first (D3)", async ({ page }) => {
  await open(page, "locked");
  await page.getByTestId("lock-mixing-only").check();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("status-unlock")).toHaveText("Unlocked for mixing only");
  await command(page, { kind: "addWallet" });
  await page.getByTestId("onboarding-create").click();
  await expect(page.getByTestId("onboarding-unlockVault")).toBeVisible();
  await page.getByTestId("onboarding-unlock-field").fill("wrong");
  await page.getByTestId("onboarding-unlock-submit").click();
  await expect(page.getByTestId("onboarding-unlock-error")).toContainText("was incorrect");
  await page.getByTestId("onboarding-unlock-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("onboarding-unlock-submit").click();
  await expect(page.getByTestId("onboarding-showPhrase")).toBeVisible();
});

test("lock screen: attempts left, then the wait (IOS-012)", async ({ page }) => {
  await open(page, "locked");
  const field = page.getByTestId("lock-field");
  for (const left of [7, 6]) {
    await field.fill("wrong");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("lock-error")).toHaveText(
      `The passphrase entered for the wallet decryption was incorrect. ${left} attempts remaining`,
    );
    await expect(field).toHaveValue("");
  }
  await field.fill("wrong");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-error")).toHaveText(
    "The passphrase entered for the wallet decryption was incorrect. Wallet disabled, please try again in 1 minute",
  );
  await expect(field).toBeDisabled();
  await expect(page.getByTestId("lock-submit")).toBeDisabled();
  expect(await world(page, (w) => w.lock.failedAttempts)).toBe(3);
});

test("lock screen: a wait and the count survive a restart, eight failures leave Forgot passphrase", async ({ page }) => {
  await open(page, "locked-throttled");
  await expect(page.getByTestId("lock-error")).toHaveText("Wallet disabled, please try again in 1 minute");
  await expect(page.getByTestId("lock-field")).toBeDisabled();
  await open(page, "locked-disabled");
  await expect(page.getByTestId("lock-error")).toHaveText(
    "Too many incorrect attempts. Restore the wallet with your recovery phrase to set a new passphrase.",
  );
  await expect(page.getByTestId("lock-field")).toBeDisabled();
  await expect(page.getByTestId("lock-forgot")).toBeVisible();
});

test("For mixing only, then the unlock sheet unlocks fully (QT-112, D12)", async ({ page }) => {
  await open(page, "locked");
  await page.getByTestId("lock-mixing-only").check();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  expect(await world(page, (w) => w.lock.state)).toBe("unlockedMixingOnly");
  await page.getByTestId("status-unlock").click();
  await expect(page.getByTestId("unlock-sheet-field")).toBeFocused();
  await page.getByTestId("unlock-sheet-field").fill("wrong");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("unlock-sheet-error")).toContainText("7 attempts remaining");
  await page.getByTestId("unlock-sheet-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("unlock-sheet")).toHaveCount(0);
  await expect(page.getByTestId("status-bar")).toContainText("Unlocked");
  await expect(page.getByTestId("status-unlock")).toHaveCount(0);
});

test("Forgot passphrase: which wallet, what happens, a mismatch, the new passphrase (IOS-013/014)", async ({ page }) => {
  await open(page, "locked-two-wallets");
  await page.getByTestId("lock-forgot").click();
  await expect(page.getByTestId("forgot-passphrase")).toBeVisible();
  await expect(page.getByTestId("forgot-wallet")).toHaveCount(2);
  await page.getByTestId("forgot-wallet").first().click();
  await expect(page.getByTestId("forgot-consequences")).toContainText("Savings will be watch-only");
  await page.getByTestId("forgot-accept").click();

  // The other wallet's phrase is valid but does not derive this one.
  await page.getByTestId("forgot-phrase-input").fill(SECOND_PHRASE);
  await page.getByTestId("forgot-continue").click();
  await page.getByTestId("forgot-new-new").fill("brand new");
  await page.getByTestId("forgot-new-repeat").fill("brand new");
  await page.getByTestId("forgot-new-submit").click();
  await expect(page.getByTestId("forgot-error")).toHaveText("This recovery phrase does not belong to the selected wallet.");
  // The phrase was dropped with the attempt.
  await expect(page.getByTestId("forgot-phrase-input")).toHaveValue("");

  // Back from the new passphrase: the phrase is typed again.
  await page.getByTestId("forgot-phrase-input").fill(MAIN_PHRASE);
  await page.getByTestId("forgot-continue").click();
  await page.getByTestId("forgot-back").click();
  await expect(page.getByTestId("forgot-phrase-input")).toHaveValue("");
  await expect(page.getByTestId("forgot-continue")).toBeDisabled();
  await page.getByTestId("forgot-phrase-input").fill(MAIN_PHRASE);
  await page.getByTestId("forgot-continue").click();
  await page.getByTestId("forgot-new-new").fill("brand new");
  await page.getByTestId("forgot-new-repeat").fill("brand new");
  await page.getByTestId("forgot-new-submit").click();
  await expect(page.getByTestId("forgot-done")).toHaveText("Your passphrase was reset.");
  await expect(page.getByTestId("forgot-watch-only")).toContainText("Savings");
  // The recovery unlocked the vault: the lock screen is gone, the sheet still says what happened.
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await page.getByTestId("forgot-close").click();
  await expect(page.getByTestId("forgot-passphrase")).toHaveCount(0);
  expect(await world(page, (w) => w.lock.walletsWithSecrets)).toEqual([FIXTURE_WALLET_ID]);
  const savings = await world(page, (w) => w.wallets.find((x) => x.summary.name === "Savings")!.summary);
  expect(savings.watchOnly).toBe(true);

  // The new passphrase unlocks; the old one does not.
  await command(page, { kind: "lock" });
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-error")).toContainText("was incorrect");
  await page.getByTestId("lock-field").fill("brand new");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
});

test("Forgot passphrase is hidden when the vault holds no wallet's secrets", async ({ page }) => {
  await open(page, "funded");
  await page.evaluate(() => {
    const fixture = window.__dwFixture!;
    fixture.world.lock = { ...fixture.world.lock, walletsWithSecrets: [] };
  });
  await command(page, { kind: "lock" });
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await expect(page.getByTestId("lock-forgot")).toHaveCount(0);
});

test("Show Recovery Phrase: the passphrase, then the words; closing drops them (IOS-003/108)", async ({ page }) => {
  await open(page, "two-wallets");
  await command(page, { kind: "recoveryPhrase" });
  await expect(page.getByTestId("recovery-phrase")).toBeVisible();
  // Protected from the sheet's start, before the phrase can show (D15).
  await expect.poll(() => secretShown(page)).toBe(true);
  // The window's wallet is chosen; the warning comes first.
  await expect(page.getByTestId("recovery-passphrase-field")).toBeFocused();
  await page.getByTestId("recovery-passphrase-field").fill("wrong");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("recovery-passphrase-error")).toContainText("was incorrect");
  await expect(page.getByTestId("recovery-words")).toHaveCount(0);
  await page.getByTestId("recovery-passphrase-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("recovery-words").locator(".phrase-word")).toHaveText(MAIN_PHRASE.split(" "));
  await expect.poll(() => secretShown(page)).toBe(true);
  await page.getByTestId("recovery-done").click();
  await expect(page.getByTestId("recovery-phrase")).toHaveCount(0);
  await expect.poll(() => secretShown(page)).toBe(false);

  // The other wallet's, from Settings.
  await page.getByTestId("toolbar-wallet-picker").selectOption(SECOND_WALLET_ID);
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-recovery-phrase").click();
  await page.getByTestId("recovery-passphrase-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("recovery-words").locator(".phrase-word")).toHaveText(SECOND_PHRASE.split(" "));
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("recovery-phrase")).toHaveCount(0);

  // Asked for while locked, it is dropped.
  await command(page, { kind: "lock" });
  await command(page, { kind: "recoveryPhrase" });
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await expect(page.getByTestId("recovery-phrase")).toHaveCount(0);
});

/** Each surface's states, each checked with axe (one test per surface and theme). */
const AXE_SURFACES: Record<string, (page: Page, theme: Theme) => Promise<void>> = {
  create: async (page, theme) => {
    await open(page, "no-wallet", theme);
    await axe(page);
    await page.getByTestId("onboarding-create").click();
    await axe(page);
    await choosePassphrase(page, "Fixture pass 42!");
    await page.getByTestId("onboarding-showPhrase").waitFor();
    await axe(page);
    await page.getByTestId("onboarding-written-down").check();
    await page.getByTestId("onboarding-phrase-continue").click();
    await page.getByTestId("onboarding-verifyPhrase").waitFor();
    await axe(page);
    await verify(page, CREATED_12);
    await page.getByTestId("onboarding-done").waitFor();
    await axe(page);
  },
  restore: async (page, theme) => {
    await open(page, "no-wallet-vault", theme);
    await page.getByTestId("onboarding-restore").click();
    await page.getByTestId("restore-input").fill("enjoy congxx layer");
    await page.getByTestId("restore-problem").waitFor();
    await axe(page);
    await page.getByTestId("restore-input").fill(CORE_ONLY_PHRASE);
    await page.getByTestId("restore-valid").waitFor();
    await page.getByTestId("restore-continue").click();
    await page.getByTestId("restore-scan-date").check();
    await axe(page);
  },
  "lock and Forgot passphrase": async (page, theme) => {
    await open(page, "locked-disabled", theme);
    await axe(page);
    await page.getByTestId("lock-forgot").click();
    await page.getByTestId("forgot-phrase-input").waitFor();
    await axe(page);
    await open(page, "locked-two-wallets", theme);
    await page.getByTestId("lock-forgot").click();
    await page.getByTestId("forgot-wallet").first().waitFor();
    await axe(page);
    await page.getByTestId("forgot-wallet").first().click();
    await axe(page);
  },
  "recovery phrase, unlock sheet and Settings": async (page, theme) => {
    await open(page, "two-wallets", theme);
    await command(page, { kind: "recoveryPhrase" });
    await page.getByTestId("recovery-passphrase-field").fill(FIXTURE_PASSPHRASE);
    await page.keyboard.press("Enter");
    await page.getByTestId("recovery-words").waitFor();
    await axe(page);
    await page.keyboard.press("Escape");
    await command(page, { kind: "navigate", route: "settings" });
    await axe(page);
    await open(page, "locked", theme);
    await page.getByTestId("lock-mixing-only").check();
    await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
    await page.keyboard.press("Enter");
    await page.getByTestId("status-unlock").click();
    await page.getByTestId("unlock-sheet-field").waitFor();
    await axe(page);
  },
};

for (const theme of THEMES) {
  for (const [surface, run] of Object.entries(AXE_SURFACES)) {
    test(`axe ${theme}: ${surface}, 0 serious issues`, async ({ page }) => {
      await run(page, theme);
    });
  }
}
