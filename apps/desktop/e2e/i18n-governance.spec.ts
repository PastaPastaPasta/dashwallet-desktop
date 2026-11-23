import { expect, test, type Locator, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Masternodes ▸ Governance under the two pseudo-locales (./pseudo.ts): the
// clock and budget, the list and its states, the pane, the vote sheet, Governance numbers, Create
// proposal, Mine and the status bar's voting item, reached as governance.spec.ts reaches them.

/** Amounts, counts, proposal names, addresses, links, dates and the fixture's masternodes. */
const DATA = [
  /^[+−-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^\u2068?[a-z0-9_-]+\u2069?$/,
  /^[1-9A-HJ-NP-Za-km-z]{4}…[1-9A-HJ-NP-Za-km-z]{4}$/,
  /^[1-9A-HJ-NP-Za-km-z]{30,}$/,
  /^(https:\/\/)?[\w.-]+\.\w+\/\S*$/,
  /^\u2068?[A-Z][a-z]{2} \d{1,2}(, \d{4})?\u2069?$/,
  /^—$/,
  /^\{[\s\S]*\}$/,
];

const open = async (page: Page, scenario: string, theme: Theme, lang: string) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
};

async function toGovernance(page: Page) {
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
}

async function toCreate(page: Page) {
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill("desktop-translations");
  await page.getByTestId("create-url").fill("https://www.dashcentral.org/p/desktop-translations");
  await page.getByTestId("create-continue").click();
  await expect(page.getByTestId("create-step-2")).toBeVisible();
}

async function toReview(page: Page) {
  await toCreate(page);
  await page.getByTestId("create-count").selectOption("3");
  await page.getByTestId("create-amount").fill("12.5");
  await page.getByTestId("create-continue").click();
  await expect(page.getByTestId("create-review")).toBeVisible();
}

async function toVote(page: Page) {
  await page.getByTestId("proposal-row-dashcon-2027-venue").click();
  await page.getByTestId("proposal-vote").click();
  await expect(page.getByTestId("vote-nodes")).toBeVisible();
  await page.getByTestId("vote-outcome-yes").click();
}

interface State {
  scenario: string;
  roots: string[];
  go?: (page: Page) => Promise<void>;
}

const LIST = ["governance-actions", "governance-clock", "governance-toolbar", "proposal-list", "proposal-pane"];
const CREATE = ["create-sheet"];

export const STATES: Record<string, State> = {
  list: { scenario: "governance", roots: LIST },
  syncing: { scenario: "governance-syncing", roots: ["governance-clock", "governance-syncing", "mine-list"] },
  failed: { scenario: "governance-failed", roots: ["governance-failed", "mine-list"] },
  disabled: { scenario: "governance-disabled", roots: ["governance-disabled"] },
  "browsing-only": {
    scenario: "governance-no-masternodes",
    roots: ["governance-browsing-only", "proposal-pane"],
    go: (p) => expect(p.getByTestId("governance-browsing-only")).toBeVisible(),
  },
  "watch-only": { scenario: "governance-watch-only", roots: ["governance-actions", "governance-notice-watch-only", "proposal-pane"] },
  banned: { scenario: "governance-banned", roots: ["proposal-pane"] },
  empty: { scenario: "governance-empty", roots: ["proposals-empty"] },
  mine: {
    scenario: "governance-submit-fails",
    roots: ["mine-list"],
    go: async (p) => {
      await p.getByTestId("governance-filter-mine").click();
      await p.getByTestId("mine-submit-desktop-wallet-i18n").click();
      await expect(p.getByTestId("mine-error-desktop-wallet-i18n")).toBeVisible();
    },
  },
  numbers: {
    scenario: "governance",
    roots: ["numbers-sheet"],
    go: async (p) => {
      await p.getByTestId("governance-more").click();
      await p.getByTestId("governance-more-numbers").click();
      await expect(p.getByTestId("numbers-cycle")).toBeVisible();
    },
  },
  vote: { scenario: "governance-vote-refused", roots: ["vote-sheet"], go: toVote },
  "vote-result": {
    scenario: "governance-vote-refused",
    roots: ["vote-sheet"],
    go: async (p) => {
      await toVote(p);
      await p.getByTestId("vote-passphrase").fill("demo");
      await p.getByTestId("vote-send").click();
      await expect(p.getByTestId("vote-result")).toBeVisible();
    },
  },
  "create-details": {
    scenario: "governance",
    roots: CREATE,
    go: async (p) => {
      await p.getByTestId("governance-create").click();
      await p.getByTestId("create-name").fill("Bad Name");
      await p.getByTestId("create-continue").click();
      await expect(p.getByTestId("create-error-url")).toBeVisible();
    },
  },
  "create-payments": {
    scenario: "governance",
    roots: CREATE,
    go: async (p) => {
      await toCreate(p);
      await p.getByTestId("create-count").selectOption("3");
      await p.getByTestId("create-amount").fill("12.5");
      await expect(p.getByTestId("create-total")).toContainText("37");
    },
  },
  "create-review": { scenario: "governance", roots: CREATE, go: toReview },
  "create-fee-changed": {
    scenario: "governance",
    roots: CREATE,
    go: async (p) => {
      await toReview(p);
      await p.evaluate(() => (window.__dwFixture!.world.governance.networkFee = 4_500));
      await p.getByTestId("create-passphrase").fill("demo");
      await p.getByTestId("create-pay").click();
      await expect(p.getByTestId("create-fee-changed")).toBeVisible();
    },
  },
  "create-already-made": {
    scenario: "governance",
    roots: CREATE,
    go: async (p) => {
      await p.evaluate(() => (window.__dwFixture!.world.governance.createFailsAfterPaying = true));
      await toReview(p);
      await p.getByTestId("create-passphrase").fill("demo");
      await p.getByTestId("create-pay").click();
      await expect(p.getByTestId("create-already-made")).toBeVisible();
    },
  },
  "status-item": { scenario: "governance", roots: ["status-governance"] },
};

const GOV_ONLY = new Set(["status-item"]);

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, s] of Object.entries(STATES))
      test(`Governance ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `governance-${name}`,
          roots: s.roots,
          data: DATA,
          open: async (lang) => {
            await open(page, s.scenario, theme, lang);
            if (GOV_ONLY.has(name)) await expect(page.getByTestId("status-governance")).toBeVisible();
            else await toGovernance(page);
            await s.go?.(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("Governance follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "governance", "light", "en");
  await toGovernance(page);
  await expect(page.getByTestId("governance-create")).toHaveText("Create proposal");
  await expect(page.getByTestId("status-governance")).toHaveText("Oct 10 votes: about 1 d 7 h left");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" } as never));
  await page.getByTestId("settings-tabs-general").click();
  await page.getByTestId("settings-language").selectOption("en-XA");
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("governance-create")).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("governance-clock-title")).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("status-governance")).toHaveText(/^⟦.+⟧$/);
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

/** A number, then its unit: the number is drawn first. */
const AMOUNT = /(\p{Nd})[\p{Nd}.,٫٬]*[⁦-⁩‪-‮]* (t?DASH)/u;

async function inOrder(root: Locator, what: string, re: RegExp = AMOUNT) {
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

test.describe("amounts keep their order under ar-XB", () => {
  const go = async (page: Page, scenario = "governance") => {
    await open(page, scenario, "light", "ar-XB");
    await toGovernance(page);
  };

  test("the proposal fee in the warning, and the review's amounts", async ({ page }) => {
    await go(page);
    await toReview(page);
    await inOrder(page.getByTestId("create-fee-warning"), "the fee warning");
    for (const id of ["create-review-payments", "create-review-fee", "create-review-total"])
      for (const amount of await page.getByTestId(id).locator(".amount").all()) await glyphAfter(amount, id);
    await inOrder(page.getByTestId("create-pay"), "the pay button");
  });

  test("a changed network fee and the new total", async ({ page }) => {
    await go(page);
    await toReview(page);
    await page.evaluate(() => (window.__dwFixture!.world.governance.networkFee = 4_500));
    await page.getByTestId("create-passphrase").fill("demo");
    await page.getByTestId("create-pay").click();
    await inOrder(page.getByTestId("create-fee-changed"), "the changed fee");
  });

  test("the budget notice on Payments", async ({ page }) => {
    await go(page);
    await toCreate(page);
    await inOrder(page.getByTestId("create-budget-notice"), "the budget notice");
  });

  test("Governance numbers' budget", async ({ page }) => {
    await go(page);
    await page.getByTestId("governance-more").click();
    await page.getByTestId("governance-more-numbers").click();
    await inOrder(page.getByTestId("numbers-votes"), "the budget row");
  });

  test("an ask in the list, the pane and the clock's budget", async ({ page }) => {
    await go(page);
    await glyphAfter(page.getByTestId("proposal-row-dash-incubator-nov").locator(".amount").first(), "the row");
    await glyphAfter(page.getByTestId("proposal-pane").locator(".mg-facts .amount").first(), "the pane");
    await glyphAfter(page.getByTestId("governance-budget").locator(".amount").first(), "the budget");
  });

  test("an ask is read amount first: in the list and the vote sheet", async ({ page }) => {
    /** Under a right-to-left language the amount is read first: it sits right of "× 3 payments". */
    const amountFirst = async (root: Locator, what: string) => {
      const [amount, times] = await root.evaluate((el) => {
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          const at = (n.textContent ?? "").indexOf("×");
          if (at < 0) continue;
          const r = document.createRange();
          r.setStart(n, at);
          r.setEnd(n, at + 1);
          return [el.querySelector(".amount")!.getBoundingClientRect().left, r.getBoundingClientRect().left];
        }
        return [0, 0];
      });
      expect(amount, what).toBeGreaterThan(times);
    };
    await go(page);
    await amountFirst(page.getByTestId("proposal-row-dash-incubator-nov").locator(".mg-row-sub"), "the row");
    await page.getByTestId("proposal-row-dash-incubator-nov").click();
    await page.getByTestId("proposal-vote").click();
    await amountFirst(page.getByTestId("vote-sheet").locator("p").first(), "the vote sheet");
  });

  test("a margin keeps its sign before its number", async ({ page }) => {
    await go(page);
    await inOrder(page.getByTestId("proposal-row-dash-incubator-nov"), "the margin", /([+−])(\p{Nd})/u);
  });
});
