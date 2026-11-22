import { expect, test, type Page } from "@playwright/test";
import type { ToolsTab } from "../src/api/generated/ToolsTab";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, openToolsTab, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Tools under the two pseudo-locales (./pseudo.ts): all eight tabs,
// Information's warning banner and DashPay card, Repair's confirmation and GrantSheet, Address
// Book's editor, QR and empty states, PSBT operations, Coin Selection's summary, and No wallet.
// States are reached as tools.spec.ts and dashpay-info.spec.ts reach them.

const UNSIGNED = Buffer.from("psbt\xffunsigned", "latin1").toString("base64");

const WALLET = "a1".repeat(32);

async function load(page: Page, theme: Theme, lang: string, scenario: string, extra = "") {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}${extra}`));
  await page.getByTestId("toolbar").waitFor();
}

async function openTab(page: Page, theme: Theme, lang: string, tab: ToolsTab, scenario = "funded", extra = "") {
  await load(page, theme, lang, scenario, extra);
  await openToolsTab(page, tab);
}

/** Data shown by Tools tabs: addresses, hex hashes, versions, user agents, network names, dates,
 * units, and fixture labels/names. Uses `\p{Nd}` for WebKit's Arabic-Indic digits under `ar-XB`. */
const TOOLS_DATA = [
  /^[yX789][1-9A-HJ-NP-Za-km-z]{20,}$/,
  /^[yX789][1-9A-HJ-NP-Za-km-z]+…[1-9A-HJ-NP-Za-km-z]+$/,
  /^[0-9a-f]{16,}$/i,
  /^(main|test|regtest|devnet-\S+)$/,
  /^\/[\w .:-]+\/$/,
  /^\p{Nd}+(\.\p{Nd}+)? (B|KB|MB|GB|ms)$/u,
  /^(\p{Nd}+ [dhms])( \p{Nd}+ [dhms])*$/u,
  /^(Sun|Mon|Tue|Wed|Thu|Fri|Sat) (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \p{Nd}{1,2} \p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2} \p{Nd}{4}$/u,
  /^\p{Nd}+, ((Sun|Mon|Tue|Wed|Thu|Fri|Sat) (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \p{Nd}{1,2} \p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2} \p{Nd}{4}, )?[0-9a-f]{64}$/iu,
  /^fixture$/,
  /^(Alice|Bob's shop|Payroll|Savings|Savings \(mixed\)|alice|bob|carol)$/,
];

/** Still English, from shared components (`components/emptyStates.ts`, `components/coins.tsx`,
 * `components/passphrase.tsx`, `components/sheet.tsx`, `components/detail.tsx`). */
const EMPTY_ADDRESS_BOOK = [
  /^No sending addresses yet$/,
  /^Save an address you pay often\.$/,
  /^Add address$/,
];
const COINS_SHARED = [
  /^Status$/,
  /^Quantity$/,
  /^Amount$/,
  /^Fee$/,
  /^After fee$/,
  /^Bytes$/,
  /^Change$/,
  /^Locked$/,
  /^Unconfirmed$/,
  /^Reserved$/,
  /^Mixed, \p{Nd}+ rounds$/u,
  /^(Coins of this wallet|Select)$/,
  /^\p{Nd}{1,2}\/\p{Nd}{1,2}\/\p{Nd}{2,4},? \p{Nd}{1,2}:\p{Nd}{2}( [AP]M)?$/u,
];
const GRANT_SHARED = [/^Passphrase$/];

/** dash-qt's columns (D22): these tables scroll sideways inside their card in English too. */
const PEERS_TABLE = '.table-card:has([data-testid="peers-table"])';
const COIN_TABLE = '.table-card:has([data-testid="coin-table"])';

/** Works around #240 (coins.tsx's screen-reader-only headers widen the page in English) until #240 lands. */
const CONTAIN_ENGLISH_COIN_TABLE_SR_ONLY_HEADERS = `${COIN_TABLE} { position: relative; }`;

const STATES: Record<string, { open: (page: Page, theme: Theme, lang: string) => Promise<void>; roots?: string[]; data?: RegExp[]; panes?: string[]; scrollers?: string[] }> = {
  information: {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "information", "dashpay-info-spv");
      await expect(page.getByTestId("information-dashpay-state")).toHaveAttribute("data-state", "running");
    },
  },
  "information-warning": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "information", "tools-warnings");
      await expect(page.getByTestId("information-banner")).toBeVisible();
    },
  },
  "information-error": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "information", "dashpay-info-spv", "&fail=node_info:none");
      await expect(page.getByTestId("information-error")).toBeVisible();
    },
  },
  console: {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "console");
      await expect(page.getByTestId("console-log")).toBeVisible();
    },
  },
  peers: {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "peers");
      const first = page.getByTestId("peers-table").locator("tbody tr").first();
      await expect(first).toBeVisible();
      await first.getByRole("button").click();
      await expect(page.getByTestId("peer-detail")).toBeVisible();
    },
    scrollers: [PEERS_TABLE],
  },
  repair: {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "repair", "tools-stuck-funding");
      await expect(page.getByTestId("repair-stuck")).toBeVisible();
    },
  },
  "repair-confirm": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "repair", "tools-stuck-funding");
      await page.getByTestId("repair-reset-chain").click();
      await expect(page.getByTestId("repair-confirm")).toBeVisible();
    },
    roots: ["page-tools", "repair-confirm"],
    panes: ['[data-testid="repair-confirm"]'],
  },
  "repair-stuck-shield": {
    open: async (page, theme, lang) => {
      await load(page, theme, lang, "coinjoin-move");
      await page.evaluate(() => {
        window.__dwFixture!.world.coinjoin.drainUnknown = "stuck";
      });
      await openToolsTab(page, "repair");
      await expect(page.getByTestId("repair-stuck-shields")).toBeVisible();
      await page.getByTestId("repair-resolve-shield/0/funding").click();
      await expect(page.getByTestId("repair-confirm")).toBeVisible();
    },
    roots: ["page-tools", "repair-confirm"],
    panes: ['[data-testid="repair-confirm"]'],
  },
  "repair-grant": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "repair", "tools-stuck-funding");
      await page.getByTestId("repair-stuck").getByRole("button").first().click();
      await page.getByTestId("repair-confirm-ok").click();
      await expect(page.getByTestId("grant-sheet")).toBeVisible();
    },
    roots: ["page-tools", "grant-sheet"],
    panes: ['[data-testid="grant-sheet"]'],
    data: GRANT_SHARED,
  },
  "address-book": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "addressBook");
      await expect(page.getByTestId("address-book-table")).toBeVisible();
    },
  },
  "address-book-editor": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "addressBook");
      await page.getByTestId("address-book-new").click();
      await page.getByTestId("address-book-address").fill("nonsense");
      await page.getByTestId("address-book-save").click();
      await expect(page.getByTestId("address-book-error")).toBeVisible();
    },
    roots: ["page-tools", "address-book-editor"],
    panes: ['[data-testid="address-book-editor"]'],
  },
  "address-book-qr": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "addressBook");
      await page.getByTestId("address-book-receive").click();
      const payroll = page.getByTestId("address-book-table").locator("tr", { hasText: "Payroll" });
      await payroll.locator("[data-testid^=address-book-qr-]").click();
      await expect(page.getByTestId("address-book-qr-code")).toBeVisible();
    },
    roots: ["page-tools", "address-book-qr"],
    panes: ['[data-testid="address-book-qr"]'],
    data: [/^dash:\S+$/],
  },
  "address-book-empty": {
    open: async (page, theme, lang) => {
      await load(page, theme, lang, "funded");
      await page.evaluate((id) => {
        window.__dwFixture!.world.tools.addressBook[id] = [];
      }, WALLET);
      await openToolsTab(page, "addressBook");
      await expect(page.getByTestId("address-book-none")).toBeVisible();
    },
    data: EMPTY_ADDRESS_BOOK,
  },
  "sign-verify": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "signVerify");
      await expect(page.getByTestId("sign-card")).toBeVisible();
    },
  },
  "sign-verify-picker": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "signVerify");
      await page.getByTestId("sign-choose").click();
      await expect(page.getByTestId("address-book-table")).toBeVisible();
    },
    roots: ["page-tools", "address-book-picker"],
    panes: ['[data-testid="address-book-picker"]'],
  },
  psbt: {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "psbt");
      await page.getByTestId("psbt-paste").fill(UNSIGNED);
      await page.getByTestId("psbt-load-text").click();
      await expect(page.getByTestId("psbt-operations")).toBeVisible();
    },
    data: [/^(?=.*[0-9+/=])[A-Za-z0-9+/]{16,}={0,2}$/],
  },
  "psbt-error": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "psbt");
      await page.getByTestId("psbt-paste").fill("not base64!");
      await page.getByTestId("psbt-load-text").click();
      await expect(page.getByTestId("psbt-error")).toBeVisible();
    },
    data: [/^not base64!$/],
  },
  "coin-selection": {
    open: async (page, theme, lang) => {
      await openTab(page, theme, lang, "coinSelection", "tools-coinjoin");
      await expect(page.getByTestId("coin-table")).toBeVisible();
      await page.getByTestId("coins-select-all").click();
      await expect(page.getByTestId("coins-selection")).toBeVisible();
      await page.addStyleTag({ content: CONTAIN_ENGLISH_COIN_TABLE_SR_ONLY_HEADERS });
    },
    data: COINS_SHARED,
    scrollers: [COIN_TABLE],
  },
  "no-wallet": {
    open: async (page, theme, lang) => {
      await load(page, theme, lang, "no-wallet");
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" }));
      await expect(page.getByTestId("tools-repair-no-wallet")).toBeVisible();
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Tools ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        const data = [...TOOLS_DATA, ...(state.data ?? [])];
        await checkArea(page, info, theme, pseudo, {
          name: `tools-${name}`,
          roots: state.roots ?? ["page-tools"],
          data,
          panes: state.panes,
          scrollers: state.scrollers,
          open: (lang) => state.open(page, theme, lang),
        });
        expect(errors).toEqual([]);
      });
