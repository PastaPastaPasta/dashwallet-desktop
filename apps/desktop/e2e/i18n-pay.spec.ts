import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO, type AreaCheck } from "./pseudo";
import { PAYEE } from "./wallet";

// Pay (MP-14) under the two pseudo-locales (./pseudo.ts): To an address empty, filled and refused, its
// coin control and unsigned review, the confirm sheet, the result sent and not sent, To a contact's
// picker, form and confirm, and Pay from Platform's form and confirm.

const PLATFORM_PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";

/** Data in Pay's states: addresses (whole or shortened), transaction ids, amounts with their unit, and the fixture's labels. */
const ADDRESSES = [/^[Xy][1-9A-HJ-NP-Za-km-z]{25,34}$/, /^[Xy][1-9A-HJ-NP-Za-km-z]+…[1-9A-HJ-NP-Za-km-z]+$/, /^tdash1[0-9a-z]+$/, /^[0-9a-f]{64}$/, /^[\p{Nd}.,]+ t?DASH$/u, /^Payroll$/];

/** The fixture's contacts. */
const CONTACTS = [/^(Carol|Grace Hopper|Bob Builder|Dave|dave|Erin|heidi)$/, /^\p{L}{1,2}$/u];

/** Still English: a contact's payment note comes from contacts/pane.ts. */

const world = (page: Page, fn: string) => page.evaluate(`(${fn})(window.__dwFixture.world)`);

async function payPage(page: Page, scenario: string, theme: Theme, lang: string) {
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-pay").click();
}

async function toAddress(page: Page, theme: Theme, lang: string, scenario = "funded") {
  await payPage(page, scenario, theme, lang);
  await expect(page.getByTestId("pay-address-form")).toBeVisible();
}

async function filled(page: Page, theme: Theme, lang: string, broadcast?: string) {
  await toAddress(page, theme, lang);
  if (broadcast) await world(page, `(w) => (w.send.broadcast = "${broadcast}")`);
  await page.getByTestId("pay-address").fill(PAYEE);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await page.getByTestId("pay-amount").fill("0.5");
  await page.getByTestId("pay-fee-line").waitFor();
}

async function confirm(page: Page) {
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeVisible();
}

async function sent(page: Page, outcome: "sent" | "failed") {
  await confirm(page);
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId(`pay-result-${outcome}`)).toBeVisible();
}

async function contactForm(page: Page, theme: Theme, lang: string) {
  await payPage(page, "dashpay-registered", theme, lang);
  await page.getByTestId("pay-contacts").click();
  await page.getByTestId("pay-picker-row").filter({ hasText: "Carol" }).click();
  await expect(page.getByTestId("pay-amount")).toBeVisible();
}

async function platform(page: Page, theme: Theme, lang: string) {
  await toAddress(page, theme, lang);
  await page.getByTestId("pay-address").fill(PLATFORM_PAYEE);
  await page.getByTestId("platform-send-intro").waitFor();
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-fee-line").waitFor();
}

type State = Omit<AreaCheck, "open"> & { open: (page: Page, theme: Theme, lang: string) => Promise<void> };

const STATES: State[] = [
  { name: "pay-address-empty", roots: ["pay-address-form"], open: (page, theme, lang) => toAddress(page, theme, lang) },
  { name: "pay-address-filled", roots: ["pay-address-form"], data: ADDRESSES, open: (page, theme, lang) => filled(page, theme, lang) },
  {
    name: "pay-address-error",
    roots: ["pay-address-form"],
    open: async (page, theme, lang) => {
      await toAddress(page, theme, lang);
      await page.getByTestId("pay-address").fill("not an address");
      await page.getByTestId("pay-amount").fill("0.000001");
      await expect(page.getByTestId("pay-address")).toHaveAttribute("aria-invalid", "true");
    },
  },
  {
    name: "pay-amount-malformed",
    roots: ["pay-address-form"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await filled(page, theme, lang);
      await page.getByTestId("pay-amount").fill("abc");
      await expect(page.getByTestId("pay-amount")).toHaveAttribute("aria-invalid", "true");
    },
  },
  {
    name: "pay-coin-control",
    roots: ["pay-address-form"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await filled(page, theme, lang);
      await page.getByTestId("pay-advanced").locator("summary").click();
      await page.getByTestId("coin-choice-selected").check();
      await page.getByTestId("coin-table").locator("input[type=checkbox]").first().check();
      await page.getByTestId("coin-summary").waitFor();
      await page.getByTestId("fee-custom").check();
      await page.getByTestId("fee-rate").fill("abc");
      await page.getByTestId("change-custom").check();
      await expect(page.getByTestId("fee-choice").locator(".field-error")).toBeVisible();
    },
  },
  {
    name: "pay-psbt",
    roots: ["pay-address-form", "pay-confirm"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await filled(page, theme, lang);
      await page.getByTestId("pay-advanced-psbt").locator("summary").click();
      await page.getByTestId("pay-create-psbt").click();
      await expect(page.getByTestId("pay-confirm-unsigned")).toBeVisible();
    },
  },
  {
    name: "pay-confirm",
    roots: ["pay-confirm"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await filled(page, theme, lang);
      await confirm(page);
    },
  },
  {
    // U-06b: twelve fully mixed coins, of which paying 1.05 spends eleven, so the confirm sheet shows the count, the warning and the 0.05 left-over warning.
    name: "pay-confirm-mixed",
    roots: ["pay-confirm"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await toAddress(page, theme, lang);
      await page.evaluate(`(() => {
        const f = window.__dwFixture;
        const wallet = f.world.wallets[0];
        const c = wallet.coins[0];
        for (let i = 0; i < 12; i++) wallet.coins.push({ ...c, outpoint: { ...c.outpoint, vout: 40 + i }, amount: 10_000_100, spendable: true, confirmations: 10, coinjoinDenominated: true, coinjoinRounds: 4 });
        f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: wallet.summary.walletId });
      })()`);
      await page.getByTestId("pay-address").fill(PAYEE);
      await page.getByTestId("pay-amount").fill("1.05");
      await page.getByTestId("pay-advanced").locator("summary").click();
      await page.getByTestId("coin-choice-mixed").check();
      await page.getByTestId("pay-fee-line").waitFor();
      await confirm(page);
      await expect(page.getByTestId("pay-confirm-mixed-warning")).toBeVisible();
      await expect(page.getByTestId("pay-confirm-mixed-left-over")).toBeVisible();
    },
  },
  {
    name: "pay-result-sent",
    roots: ["pay-result"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await filled(page, theme, lang);
      await sent(page, "sent");
    },
  },
  {
    name: "pay-result-failed",
    roots: ["pay-result"],
    open: async (page, theme, lang) => {
      await filled(page, theme, lang, "rejected");
      await sent(page, "failed");
    },
  },
  {
    name: "pay-contact-picker",
    roots: ["pay-recipient-popover"],
    data: CONTACTS,
    open: async (page, theme, lang) => {
      await payPage(page, "dashpay-registered", theme, lang);
      await page.getByTestId("pay-contacts").click();
      await expect(page.getByTestId("pay-picker-contacts")).toBeVisible();
    },
  },
  {
    name: "pay-contact-form",
    roots: ["pay-contact-form"],
    // The From step's balances (C-funding P16).
    data: [...CONTACTS, /^[\p{Nd}.,]+ t?DASH$/u],
    open: async (page, theme, lang) => {
      await contactForm(page, theme, lang);
      await page.getByTestId("pay-amount").fill("0.02");
      await page.getByTestId("pay-fee-line").waitFor();
    },
  },
  {
    name: "pay-contact-confirm",
    roots: ["pay-confirm"],
    data: CONTACTS,
    open: async (page, theme, lang) => {
      await contactForm(page, theme, lang);
      await page.getByTestId("pay-amount").fill("0.02");
      await confirm(page);
    },
  },
  { name: "pay-platform", roots: ["pay-address-form"], data: ADDRESSES, open: (page, theme, lang) => platform(page, theme, lang) },
  {
    name: "pay-platform-confirm",
    roots: ["platform-send-sheet"],
    data: ADDRESSES,
    open: async (page, theme, lang) => {
      await platform(page, theme, lang);
      await page.getByTestId("pay-continue").click();
      await page.getByTestId("platform-send-confirm").waitFor();
    },
  },
  {
    name: "pay-platform-sent",
    roots: ["platform-send-sheet"],
    open: async (page, theme, lang) => {
      await platform(page, theme, lang);
      await page.getByTestId("pay-continue").click();
      await page.getByTestId("platform-send-go").click();
      await authorize(page);
      await page.getByTestId("platform-send-outcome-title").waitFor();
    },
  },
];

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const state of STATES)
      test(`${state.name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, { ...state, open: (lang) => state.open(page, theme, lang) });
        expect(errors).toEqual([]);
      });
