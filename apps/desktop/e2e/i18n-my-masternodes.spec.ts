import { expect, test, type Locator, type Page } from "@playwright/test";
import { CLUB, HOME, OLD } from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Masternodes ▸ My masternodes under the two pseudo-locales (./pseudo.ts): the
// summary, the groups and rows, the pane (facts, payouts, keys, the registrar line), rename and the
// page's states, reached as masternodes.spec.ts reaches them.

/** Amounts, counts, the fixture's labels, services, hashes, addresses and dates. */
const DATA = [
  /^[+−-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^⁨?(Home|Office|Old) node⁩?$/,
  /^⁨?[\d.]+:\d+⁩?$/,
  /^⁨?[0-9a-f]{4}…[0-9a-f]{4}⁩?$/,
  /^[1-9A-HJ-NP-Za-km-z]{4}…[1-9A-HJ-NP-Za-km-z]{4}$/,
  /^⁨?[A-Z][a-z]{2} \d{1,2}(, \d{4})?⁩?$/,
  /^—$/,
];

const open = async (page: Page, scenario: string, theme: Theme, lang: string, extra = "") => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}${extra}`));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
};

const select = async (page: Page, hash: string) => {
  await page.getByTestId(`mn-row-${hash}`).click();
  await expect(page.getByTestId("mn-facts")).toBeVisible();
};

interface State {
  scenario: string;
  roots: string[];
  extra?: string;
  go?: (page: Page) => Promise<void>;
}

const PAGE = ["page-masternodes"];

export const STATES: Record<string, State> = {
  list: { scenario: "mn-list", roots: PAGE, go: (p) => expect(p.getByTestId("mn-banned")).toBeVisible() },
  home: { scenario: "mn-list", roots: PAGE, go: async (p) => (await select(p, HOME), await expect(p.getByTestId("mn-payouts")).toBeVisible()) },
  evonode: { scenario: "mn-list", roots: PAGE, go: async (p) => (await select(p, CLUB), await expect(p.getByTestId("mn-registrar-line")).toBeVisible()) },
  retired: {
    scenario: "mn-list",
    roots: PAGE,
    go: async (p) => {
      await p.getByTestId("mn-retired-toggle").click();
      await select(p, OLD);
    },
  },
  rename: {
    scenario: "mn-list",
    roots: ["mn-rename-sheet"],
    go: async (p) => {
      await select(p, HOME);
      await p.getByTestId("mn-more").click();
      await p.getByTestId("mn-more-rename").click();
      await expect(p.getByTestId("mn-rename-sheet")).toBeVisible();
    },
  },
  "v24-payouts": { scenario: "mn-v24-payouts", roots: PAGE, go: async (p) => (await select(p, HOME), await expect(p.getByTestId("mn-payouts-flagged")).toBeVisible()) },
  "payouts-failed": { scenario: "mn-v24-payouts", extra: "&fail=mn_payouts:none", roots: PAGE, go: async (p) => (await select(p, HOME), await expect(p.getByTestId("mn-payouts-failed")).toBeVisible()) },
  "empty-coin": { scenario: "mn-empty-coin", roots: PAGE, go: (p) => expect(p.getByTestId("mn-empty-coin")).toBeVisible() },
  syncing: { scenario: "mn-syncing", roots: PAGE, go: (p) => expect(p.getByTestId("mn-syncing")).toBeVisible() },
  failed: { scenario: "mn-list-failed", roots: PAGE, go: (p) => expect(p.getByTestId("mn-list-failed")).toBeVisible() },
  "watch-only": { scenario: "mn-watchonly", roots: PAGE, go: (p) => select(p, HOME) },
  locked: { scenario: "mn-locked", roots: PAGE, go: async (p) => (await select(p, CLUB), await expect(p.getByTestId("mn-unlock")).toBeVisible()) },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, s] of Object.entries(STATES))
      test(`My masternodes ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `my-masternodes-${name}`,
          roots: s.roots,
          data: DATA,
          open: async (lang) => {
            await open(page, s.scenario, theme, lang, s.extra);
            await s.go?.(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("My masternodes follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "mn-list", "light", "en");
  await expect(page.getByTestId("masternodes-tabs-myMasternodes")).toHaveText("My masternodes");
  await expect(page.getByTestId("mn-list-height")).toHaveText("Masternode list at block 1,214,880");
  await expect(page.getByTestId("mn-group-retired")).toContainText("Retired (1)");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" } as never));
  await page.getByTestId("settings-tabs-general").click();
  await page.getByTestId("settings-language").selectOption("en-XA");
  await page.getByTestId("sidebar-masternodes").click();
  for (const id of ["masternodes-tabs-myMasternodes", "masternodes-tabs-names", "mn-list-height", "mn-pane-chip", "mn-retired-toggle"]) await expect(page.getByTestId(id)).toHaveText(/^⟦.+⟧$/);
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

/** The number, then what it counts: the number is drawn first. */
async function inOrder(root: Locator, what: string, re: RegExp) {
  await expect(root).toContainText(re);
  const at = await lefts(root, re);
  expect(at, what).not.toBeNull();
  expect(at![0], what).toBeLessThan(at![1]);
}

/** An amount drawn with the Dash glyph: its number comes before the glyph. */
async function glyphAfter(amount: Locator, what: string) {
  const [number, glyph] = await amount.evaluate((el) => [el.querySelector(".amount-number")!.getBoundingClientRect().left, el.querySelector(".glyph")!.getBoundingClientRect().left]);
  expect(number, what).toBeLessThan(glyph);
}

/** Under a right-to-left language an amount is read first: it sits right of `word`. */
async function amountFirst(root: Locator, word: RegExp, what: string) {
  const [amount, after] = await root.evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        if (n.parentElement?.closest(".amount")) continue;
        const m = re.exec(n.textContent ?? "");
        if (!m) continue;
        const r = document.createRange();
        r.setStart(n, m.index);
        r.setEnd(n, m.index + 1);
        return [el.querySelector(".amount")!.getBoundingClientRect().left, r.getBoundingClientRect().left];
      }
      return [0, 0];
    },
    [word.source, word.flags] as const,
  );
  expect(amount, what).toBeGreaterThan(after);
}

test.describe("amounts and counts keep their order under ar-XB", () => {
  const go = (page: Page, scenario = "mn-list") => open(page, scenario, "light", "ar-XB");

  test("collateral amounts read number first, before Dash", async ({ page }) => {
    await go(page);
    await select(page, HOME);
    await inOrder(page.getByTestId("mn-roles"), "a masternode's collateral", /(\p{Nd})[\p{Nd}.,٫٬]*[⁦-⁩‪-‮]* ?[⁦-⁩‪-‮]*(Dash|hsaD)/u);
    await go(page, "mn-empty-coin");
    await inOrder(page.getByTestId("mn-empty-coin"), "the collateral coin", /(\p{Nd})[\p{Nd}.,٫٬]*[⁦-⁩‪-‮]* ?[⁦-⁩‪-‮]*(Dash|hsaD)/u);
  });

  test("the operator's share keeps its number before %", async ({ page }) => {
    await go(page);
    await select(page, HOME);
    await inOrder(page.getByTestId("mn-fact-operator-share"), "the operator's share", /(\p{Nd})[\p{Nd}.,٫٬]*[⁦-⁩‪-‮]*(%)/u);
  });

  test("payout amounts: the glyph after the number, and the total read before its payments", async ({ page }) => {
    await go(page, "mn-v24-payouts");
    await select(page, HOME);
    await glyphAfter(page.getByTestId("mn-summary-payouts").locator(".amount").first(), "the summary");
    await glyphAfter(page.getByTestId("mn-fact-last-payout").locator(".amount").first(), "the last payout");
    for (const id of ["mn-payouts-attributed", "mn-payouts-flagged"]) {
      await glyphAfter(page.getByTestId(id).locator(".amount").first(), id);
      await amountFirst(page.getByTestId(id).locator("p"), /\p{Nd}/u, id);
    }
  });
});
