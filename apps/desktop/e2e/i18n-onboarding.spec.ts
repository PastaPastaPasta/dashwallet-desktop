import { expect, test, type Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { CREATED_12, MAIN_PHRASE, SECOND_PHRASE } from "../src/fixture/vault";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Onboarding, Forgot passphrase, Show Recovery Phrase and the first-run notice under the two
// pseudo-locales (./pseudo.ts), reached as e2e/onboarding.spec.ts reaches them.

/** Still English, from the shared controls in components/passphrase.tsx and components/phrase.tsx. */
const SHARED = [/^New passphrase$/, /^Repeat the passphrase$/, /^Passphrase$/, /^Recovery phrase$/, /^BIP39 passphrase \(optional\)$/, /^The recovery phrase is valid\.$/];

/** The fixture's data: its second wallet's name and the phrase typed with an unknown word. */
const TYPED = "enjoy congxx layer";
const DATA = [/^Savings$/, new RegExp(`^${TYPED}$`)];

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);

async function welcome(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("no-wallet", theme, `&lang=${lang}`));
  await page.getByTestId("onboarding-welcome").waitFor();
}

async function choosePassphrase(page: Page, theme: Theme, lang: string) {
  await welcome(page, theme, lang);
  await page.getByTestId("onboarding-create").click();
  await page.getByTestId("onboarding-choosePassphrase").waitFor();
}

async function showPhrase(page: Page, theme: Theme, lang: string) {
  await choosePassphrase(page, theme, lang);
  await page.getByTestId("onboarding-passphrase-new").fill("Fixture pass 42!");
  await page.getByTestId("onboarding-passphrase-repeat").fill("Fixture pass 42!");
  await page.getByTestId("onboarding-passphrase-submit").click();
  await page.getByTestId("onboarding-showPhrase").waitFor();
}

/** The verify step; each slot's label carries its word's number in any language. */
async function verifyPhrase(page: Page, theme: Theme, lang: string) {
  await showPhrase(page, theme, lang);
  await page.getByTestId("onboarding-written-down").check();
  await page.getByTestId("onboarding-phrase-continue").click();
  await page.getByTestId("onboarding-verifyPhrase").waitFor();
  const words = CREATED_12.split(" ");
  const labels = await page.getByTestId("verify-slot").locator(".verify-slot-label").allTextContents();
  const latin = (l: string) => l.replace(/[\u0660-\u0669]/g, (d) => String(d.charCodeAt(0) - 0x660));
  return labels.map((l) => words[Number(latin(l).match(/\d+/)![0]) - 1]);
}

async function restore(page: Page, theme: Theme, lang: string) {
  await welcome(page, theme, lang);
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("onboarding-restorePhrase").waitFor();
}

async function forgot(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("locked-two-wallets", theme, `&lang=${lang}`));
  await page.getByTestId("lock-forgot").click();
  await page.getByTestId("forgot-wallet").first().waitFor();
}

async function forgotPhrase(page: Page, theme: Theme, lang: string) {
  await forgot(page, theme, lang);
  await page.getByTestId("forgot-wallet").first().click();
  await page.getByTestId("forgot-consequences").waitFor();
  await page.getByTestId("forgot-accept").click();
  await page.getByTestId("forgot-phrase-input").waitFor();
}

async function recovery(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("two-wallets", theme, `&lang=${lang}`));
  await page.getByTestId("hero").waitFor();
  await command(page, { kind: "recoveryPhrase" });
  await page.getByTestId("recovery-passphrase-field").waitFor();
}

type State = { name: string; roots: string[]; open: (page: Page, theme: Theme, lang: string) => Promise<void> };

const ONBOARDING = ["onboarding"];
const FORGOT = ["forgot-passphrase"];
const RECOVERY = ["recovery-phrase"];

const STATES: State[] = [
  {
    name: "onboarding-welcome",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      await welcome(page, theme, lang);
      await page.locator(".onboarding-advanced summary").click();
      await page.getByTestId("onboarding-24-words").waitFor();
    },
  },
  {
    name: "onboarding-passphrase-error",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      await choosePassphrase(page, theme, lang);
      await page.getByTestId("onboarding-passphrase-new").fill("short");
      await page.getByTestId("onboarding-passphrase-submit").click();
      await page.getByTestId("onboarding-passphrase-error").waitFor();
      await page.locator(".onboarding-advanced summary").click();
      await page.getByTestId("onboarding-skip-passphrase").waitFor();
    },
  },
  { name: "onboarding-show-phrase", roots: ONBOARDING, open: showPhrase },
  {
    name: "onboarding-verify-mistake",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      const asked = await verifyPhrase(page, theme, lang);
      await page.getByTestId("verify-chip").filter({ hasNotText: new RegExp(`^${asked[0]}$`) }).first().click();
      await expect(page.getByTestId("verify-mistake")).not.toBeEmpty();
    },
  },
  {
    name: "onboarding-done",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      for (const word of await verifyPhrase(page, theme, lang)) await page.getByTestId("verify-chip").filter({ hasText: new RegExp(`^${word}$`) }).click();
      await page.getByTestId("onboarding-done").waitFor();
    },
  },
  {
    name: "onboarding-restore-invalid-word",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      await restore(page, theme, lang);
      await page.getByTestId("restore-input").fill(TYPED);
      await page.getByTestId("restore-problem").waitFor();
    },
  },
  {
    name: "onboarding-restore-options",
    roots: ONBOARDING,
    open: async (page, theme, lang) => {
      await restore(page, theme, lang);
      await page.getByTestId("restore-input").fill(SECOND_PHRASE);
      await page.getByTestId("restore-valid").waitFor();
      await page.getByTestId("restore-continue").click();
      await page.getByTestId("onboarding-restoreOptions").waitFor();
      await page.getByTestId("restore-scan-height").check();
      await page.getByTestId("restore-height").fill("1200");
      await page.getByTestId("restore-scan-start").waitFor();
    },
  },
  { name: "forgot-wallet", roots: FORGOT, open: forgot },
  {
    name: "forgot-consequences",
    roots: FORGOT,
    open: async (page, theme, lang) => {
      await forgot(page, theme, lang);
      await page.getByTestId("forgot-wallet").first().click();
      await page.getByTestId("forgot-consequences").waitFor();
    },
  },
  {
    name: "forgot-mismatch",
    roots: FORGOT,
    open: async (page, theme, lang) => {
      await forgotPhrase(page, theme, lang);
      await page.getByTestId("forgot-phrase-input").fill(SECOND_PHRASE);
      await page.getByTestId("forgot-continue").click();
      await page.getByTestId("forgot-new-new").fill("brand new");
      await page.getByTestId("forgot-new-repeat").fill("brand new");
      await page.getByTestId("forgot-new-submit").click();
      await page.getByTestId("forgot-error").waitFor();
    },
  },
  {
    name: "forgot-done",
    roots: FORGOT,
    open: async (page, theme, lang) => {
      await forgotPhrase(page, theme, lang);
      await page.getByTestId("forgot-phrase-input").fill(MAIN_PHRASE);
      await page.getByTestId("forgot-continue").click();
      await page.getByTestId("forgot-new-new").fill("brand new");
      await page.getByTestId("forgot-new-repeat").fill("brand new");
      await page.getByTestId("forgot-new-submit").click();
      await page.getByTestId("forgot-watch-only").waitFor();
    },
  },
  { name: "recovery-confirm", roots: RECOVERY, open: recovery },
  {
    name: "recovery-shown",
    roots: RECOVERY,
    open: async (page, theme, lang) => {
      await recovery(page, theme, lang);
      await page.getByTestId("recovery-passphrase-field").fill(FIXTURE_PASSPHRASE);
      await page.keyboard.press("Enter");
      await page.getByTestId("recovery-words").waitFor();
    },
  },
  {
    name: "pre-release-notice",
    roots: ["pre-release-notice"],
    open: async (page, theme, lang) => {
      await page.goto(fixtureUrl("pre-release-notice", theme, `&lang=${lang}`));
      await page.getByTestId("pre-release-notice").waitFor();
    },
  },
];

/** The fixture's phrase words are data. */
const WORDS = new RegExp(`^(${[...new Set([...CREATED_12.split(" "), ...MAIN_PHRASE.split(" ")])].join("|")})$`);

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const state of STATES)
      test(`${state.name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, { name: state.name, roots: state.roots, data: [...SHARED, ...DATA, WORDS], open: (lang) => state.open(page, theme, lang) });
        expect(errors).toEqual([]);
      });
