import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): CoinJoin under the two pseudo-locales (./pseudo.ts): the CoinJoin page
// (mixing with expanded Session details, custom level, empty/too-little, error/disabled), the Home
// card (mixing and locked-stop), the Stop Mixing confirmation sheet, and Move mixed coins (wallet
// confirm and outcome, Shielded quote, combine >50 coins, and mixing refusal). States are reached
// as coinjoin.spec.ts and move-mixed.spec.ts reach them.

async function openHome(page: Page, scenario: string, theme: Theme, lang: string) {
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
}

async function openCoinJoin(page: Page, scenario: string, theme: Theme, lang: string) {
  await openHome(page, scenario, theme, lang);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await expect(page.getByTestId("page-coinJoin")).toBeVisible();
}

/** Formatted Dash amounts shown as plain strings (e.g. "0.200002 tDASH", "1,000 tDASH"). */
const AMOUNTS = [/^[\p{Nd}.,٫٬\s•*]+ (t?DASH|mDASH|μDASH|duffs)(, [\p{Nd}.,٫٬\s•*]+ (t?DASH|mDASH|μDASH|duffs))*$/u];

/** Still English, from components/sheet.tsx: QuestionSheet's Cancel button. */
const SHEET_CANCEL = [/^Cancel$/];

interface State {
  roots: string[];
  data?: RegExp[];
  open: (page: Page, theme: Theme, lang: string) => Promise<void>;
}

const STATES: Record<string, State> = {
  "page-mixing-details": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-mixing", theme, lang);
      await expect(page.getByTestId("coinjoin-status")).toHaveAttribute("data-state", "mixing");
      await page.getByTestId("coinjoin-details").locator("summary").click();
      await expect(page.getByTestId("coinjoin-sessions").locator("tbody tr")).toHaveCount(1);
    },
  },
  "page-empty": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "empty", theme, lang);
      await expect(page.getByTestId("coinjoin-status")).toHaveAttribute("data-state", "tooLittle");
      await page.getByTestId("coinjoin-details").locator("summary").click();
      await expect(page.getByTestId("coinjoin-sessions")).toBeVisible();
    },
  },
  "page-custom": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-custom", theme, lang);
      await expect(page.getByTestId("coinjoin-level-custom")).toBeVisible();
    },
  },
  "page-error": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-error", theme, lang);
      await expect(page.getByTestId("coinjoin-status")).toHaveAttribute("data-state", "error");
    },
  },
  "page-disabled": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-disabled", theme, lang);
      await expect(page.getByTestId("coinjoin-status")).toHaveAttribute("data-state", "disabled");
    },
  },
  "card-mixing": {
    roots: ["coinjoin-card"],
    open: async (page, theme, lang) => {
      await openHome(page, "coinjoin-mixing", theme, lang);
      await expect(page.getByTestId("coinjoin-card")).toHaveAttribute("data-state", "mixing");
    },
  },
  "card-locked": {
    roots: ["coinjoin-card"],
    open: async (page, theme, lang) => {
      await openHome(page, "coinjoin-locked", theme, lang);
      await expect(page.getByTestId("coinjoin-card")).toHaveAttribute("data-state", "locked");
    },
  },
  "stop-sheet": {
    roots: ["coinjoin-stop-sheet"],
    data: SHEET_CANCEL,
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-mixing", theme, lang);
      await page.getByTestId("coinjoin-action").click();
      await expect(page.getByTestId("coinjoin-stop-sheet")).toBeVisible();
    },
  },
  "move-wallet-confirm": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-move", theme, lang);
      await page.getByTestId("coinjoin-move-open").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
    },
  },
  "move-wallet-done": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-move", theme, lang);
      await page.getByTestId("coinjoin-move-open").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
      await page.getByTestId("move-mixed-confirm").click();
      await authorize(page);
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-outcome", "done");
    },
  },
  "move-shielded-confirm": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-move", theme, lang);
      await page.getByTestId("coinjoin-move-shield").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-destination", "shielded");
    },
  },
  "move-combine": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-move-many", theme, lang);
      await page.getByTestId("coinjoin-move-shield").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "combine");
    },
  },
  "move-mixing": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "coinjoin-move-mixing", theme, lang);
      await page.getByTestId("coinjoin-move-open").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "mixing");
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`CoinJoin ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `coinjoin-${name}`,
          roots: state.roots,
          data: [...AMOUNTS, ...(state.data ?? [])],
          open: (lang) => state.open(page, theme, lang),
        });
        expect(errors).toEqual([]);
      });
