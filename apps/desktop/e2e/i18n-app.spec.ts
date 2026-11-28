import { expect, test, type Locator, type Page } from "@playwright/test";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { checkArea, overflow, PANES, PSEUDO, untranslated } from "./pseudo";

// Localization (MP-14): the app's own text under the two pseudo-locales (./pseudo.ts): the splash,
// a refused store, a data folder others can change and StorageInUse, and the wallet list's loading and failed states, File ▸ Open URI… with a refused link, About,
// Pay's recipient field, "Links you opened" at each age boundary, amount entry in fiat and its notes, and
// Pay's fee warning.

/** Amounts, fiat values, versions, the network's name and engine text. */
const DATA = [
  /^[+-]?[\p{Nd}.,٫٬\s•*#]+( (t?DASH|mDASH|μDASH|duffs|USD))?$/u,
  /^≈? ?[$€£]?[\p{Nd}.,٫٬]+$/u,
  /^(t?DASH|USD)$/,
  /^⇅ ?$/,
  /^v?\d+\.\d+\.\d+/,
  /^fixture$/,
  /^(testnet|mainnet|devnet|regtest)$/,
  /^[\w.]+: /,
  /^vault\./,
  /^dash:\/\/\S*$/,
];

const PAYEE = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";

async function openApp(page: Page, scenario: string, theme: Theme, lang: string, extra = "") {
  await page.goto(fixtureUrl(scenario, theme, `${extra}&lang=${lang}`));
}

const toolbar = (page: Page) => page.getByTestId("toolbar").waitFor();
const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

/** Sets the saved links' ages (seconds before now, newest first) and offers them again through a lock and unlock. */
async function linkAges(page: Page, ages: number[]) {
  await page.getByTestId("saved-links").waitFor();
  await page.getByTestId("saved-links-later").click();
  await page.evaluate((ages) => {
    const w = window.__dwFixture!.world;
    const now = Math.floor(Date.now() / 1000);
    w.stash.forEach((s, i) => (s.info.stashedAt = now - ages[i]));
  }, ages);
  await command(page, { kind: "lock" });
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("saved-links").getByTestId("saved-link")).toHaveCount(ages.length);
}

async function payToAddress(page: Page) {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-address").fill(PAYEE);
}

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

export const STATES: Record<string, State> = {
  "splash-loading": { scenario: "funded", extra: "&hold=open_network", roots: ["splash"], go: (page) => page.getByTestId("splash").getByRole("status").waitFor() },
  "splash-failed": { scenario: "funded", extra: "&fail=open_network:retryAfter", roots: ["splash"], go: (page) => page.getByTestId("splash").getByRole("alert").waitFor() },
  "store-refused": { scenario: "funded", extra: "&openRefused=refusedPrevious", roots: ["store-refused"], go: (page) => page.getByTestId("store-refused").waitFor() },
  "store-not-private": { scenario: "funded", extra: "&notPrivate=groupWritable", roots: ["store-not-private"], go: (page) => page.getByTestId("store-not-private-reason").waitFor() },
  "store-retry": { scenario: "funded", extra: "&openRetry=yes", roots: ["store-retry"], go: (page) => page.getByTestId("store-retry").waitFor() },
  "upgrade-notice": { scenario: "funded", extra: "&upgradeNotice=snapshotDamaged", roots: ["upgrade-notice"], go: (page) => page.getByTestId("upgrade-notice").waitFor() },
  "storage-in-use": { scenario: "funded", extra: "&storageInUse=retry", roots: ["storage-in-use"], go: (page) => page.getByTestId("storage-in-use-retry").waitFor() },
  "storage-in-use-restart": { scenario: "funded", extra: "&storageInUse=restart", roots: ["storage-in-use"], go: (page) => page.getByTestId("storage-in-use-restart").waitFor() },
  "wallets-loading": { scenario: "funded", extra: "&hold=list_wallets", roots: ["wallets-loading"], go: (page) => page.getByTestId("wallets-loading").waitFor() },
  "wallets-failed": { scenario: "funded", extra: "&fail=list_wallets:none", roots: ["wallets-error"], go: (page) => page.getByTestId("wallets-error").waitFor() },
  "open-uri": {
    scenario: "funded",
    roots: ["open-uri"],
    go: async (page) => {
      await toolbar(page);
      await command(page, { kind: "openUri" });
      await page.getByTestId("open-uri").waitFor();
    },
  },
  about: {
    scenario: "funded",
    roots: ["about"],
    go: async (page) => {
      await toolbar(page);
      await command(page, { kind: "about" });
      await page.getByTestId("about").waitFor();
    },
  },
  "pay-recipient": {
    scenario: "dashpay-registered",
    roots: ["pay-recipient"],
    go: async (page) => {
      await toolbar(page);
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("pay-recipient").waitFor();
    },
  },
  "saved-links": { scenario: "saved-links", roots: ["saved-links"], go: (page) => linkAges(page, [5 * 60, 2 * 3600, 3 * 86_400]) },
  "saved-links-minutes": { scenario: "saved-links", roots: ["saved-links"], go: (page) => linkAges(page, [0, 60, 3599]) },
  "saved-links-hours": { scenario: "saved-links", roots: ["saved-links"], go: (page) => linkAges(page, [3600, 86_399, 86_400]) },
  "saved-links-days": { scenario: "saved-links", roots: ["saved-links"], go: (page) => linkAges(page, [120, 7200, 2 * 86_400]) },
  "fiat-active": {
    scenario: "fiat",
    roots: ["pay-address-form"],
    go: async (page) => {
      await toolbar(page);
      await payToAddress(page);
      await page.getByTestId("pay-amount-toggle").click();
      await page.getByTestId("pay-amount").fill("10.00");
      await expect(page.getByTestId("pay-amount-other")).toContainText(/\p{Nd}/u);
    },
  },
  "fiat-invalid": {
    scenario: "fiat",
    roots: ["pay-address-form"],
    go: async (page) => {
      await toolbar(page);
      await payToAddress(page);
      await page.getByTestId("pay-amount-toggle").click();
      await page.getByTestId("pay-amount").fill("1.2.3");
      await page.locator(".form-problem, .field-error").first().waitFor();
    },
  },
  "fiat-stale": {
    scenario: "fiat-stale",
    roots: ["pay-address-form"],
    go: async (page) => {
      await toolbar(page);
      await payToAddress(page);
      await page.getByTestId("pay-amount-other-why").waitFor();
    },
  },
  "pay-fee-warning": {
    scenario: "funded",
    roots: ["pay-confirm-fee-warning"],
    go: async (page) => {
      await toolbar(page);
      await payToAddress(page);
      await page.getByTestId("pay-amount").fill("0.00001");
      await expect(page.getByTestId("pay-continue")).toBeEnabled();
      await page.getByTestId("pay-continue").click();
      await page.getByTestId("pay-confirm-fee-warning").waitFor();
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`App ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `app-${name}`,
          roots: state.roots,
          data: DATA,
          open: async (lang) => {
            await openApp(page, state.scenario, theme, lang, state.extra);
            await state.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

// A refused link in Open URI. Its text is checked like the states above, but not with axe: the
// Open button fails colour contrast once the field has text, in English too (issue 323).
for (const theme of THEMES)
  for (const [lang, kind] of PSEUDO)
    test(`App open-uri-refused shows only catalog text under ${lang} and fits, ${theme}`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await openApp(page, "funded", theme, lang);
      await toolbar(page);
      await command(page, { kind: "openUri" });
      await page.getByTestId("open-uri-input").fill(`dash://${PAYEE}`);
      await page.getByTestId("open-uri-open").click();
      await page.getByTestId("open-uri-problem").waitFor();
      expect(await untranslated(page, ["open-uri"], kind, DATA)).toEqual([]);
      expect(await overflow(page, [...PANES, '[data-testid="open-uri"]'])).toEqual([]);
      expect(errors).toEqual([]);
    });

/** The left edges of `re`'s two groups in the first text node under `root` that matches it. */
async function lefts(root: Locator, re: RegExp): Promise<[number, number] | null> {
  return root.evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const m = re.exec(n.textContent ?? "");
        if (!m?.indices) continue;
        const left = (g: number) => {
          const r = document.createRange();
          r.setStart(n!, m.indices![g]![0]);
          r.setEnd(n!, m.indices![g]![0] + 1);
          return r.getBoundingClientRect().left;
        };
        return [left(1), left(2)] as [number, number];
      }
      return null;
    },
    [re.source, `${re.flags}d`] as const,
  );
}

test.describe("amounts keep their order under ar-XB", () => {
  test("entering in fiat, the DASH amount under the field comes before its unit", async ({ page }) => {
    await openApp(page, "fiat", "light", "ar-XB");
    await toolbar(page);
    await payToAddress(page);
    await page.getByTestId("pay-amount-toggle").click();
    await page.getByTestId("pay-amount").fill("10.00");
    const other = page.getByTestId("pay-amount-other");
    await expect(other).toContainText(/\p{Nd}/u);
    const at = await lefts(other, /(\p{Nd})[\p{Nd}.,٫٬]* (t?DASH)/u);
    expect(at, "the DASH amount").not.toBeNull();
    expect(at![0], "the DASH amount").toBeLessThan(at![1]);
  });
});
