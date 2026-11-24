import { expect, test, type Locator, type Page } from "@playwright/test";
import { authorize, fixtureUrl, openToolsTab, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): CoinJoin under the two pseudo-locales (./pseudo.ts): the CoinJoin page
// (mixing with expanded Session details, custom level, empty/too-little, error/disabled, status-read failure), the Home
// card (mixing and locked-stop), the Stop Mixing confirmation sheet, and Move mixed coins (wallet
// confirm and outcome, Shielded quote, each outcome of a move to Shielded and its refusals, combine
// >50 coins, and mixing refusal). States are reached as coinjoin.spec.ts and move-mixed.spec.ts
// reach them.

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

/** Move mixed coins ▸ Move to Shielded, approved; `prep` sets how the fixture's lock ends. */
async function shieldMove(page: Page, theme: Theme, lang: string, prep: string) {
  await openCoinJoin(page, "coinjoin-move", theme, lang);
  await page.evaluate(prep);
  await page.getByTestId("coinjoin-move-shield").click();
  await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
}

/** Each way a move to Shielded ends, after approval: the fixture's lock and the text it shows. */
const SHIELD_OUTCOMES: Record<string, string> = {
  added: `window.__dwFixture.world.pending.shielded.shield = "applied"`,
  waiting: `window.__dwFixture.world.pending.shielded.shield = "proofWaiting"`,
  checking: `window.__dwFixture.world.pending.shielded.shield = "checking"`,
  "not-applied": `window.__dwFixture.world.pending.shielded.shield = "notApplied"`,
};

interface State {
  roots: string[];
  data?: RegExp[];
  open: (page: Page, theme: Theme, lang: string) => Promise<void>;
}

const STATES: Record<string, State> = {
  "explainer": {
    roots: ["coinjoin-explainer"],
    open: async (page, theme, lang) => {
      await openCoinJoin(page, "funded", theme, lang);
      await page.getByTestId("coinjoin-explainer-open").click();
      await expect(page.getByTestId("coinjoin-explainer")).toBeVisible();
    },
  },
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
  "page-read-failed": {
    roots: ["page-coinJoin"],
    open: async (page, theme, lang) => {
      await page.goto(fixtureUrl("funded", theme, `&fail=coinjoin_status:none&lang=${lang}`));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
      await expect(page.getByTestId("coinjoin-status")).toHaveAttribute("data-state", "readFailed");
      await expect(page.getByTestId("coinjoin-status-title")).not.toHaveText("Could not read the mixing status");
      await expect(page.getByTestId("coinjoin-action")).not.toHaveText("Try again");
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
  "move-shielded-added": {
    roots: ["move-mixed-sheet"],
    data: [/^fixture: .+$/],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, SHIELD_OUTCOMES["added"]);
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-outcome", "shielded");
    },
  },
  "move-shielded-waiting": {
    roots: ["move-mixed-sheet"],
    data: [/^fixture: .+$/],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, SHIELD_OUTCOMES["waiting"]);
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-outcome", "shielded");
    },
  },
  "move-shielded-checking": {
    roots: ["move-mixed-sheet"],
    data: [/^fixture: .+$/],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, SHIELD_OUTCOMES["checking"]);
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-outcome", "shielded");
    },
  },
  "move-shielded-not-applied": {
    roots: ["move-mixed-sheet"],
    data: [/^fixture: .+$/],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, SHIELD_OUTCOMES["not-applied"]);
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-outcome", "shielded");
    },
  },
  "move-shielded-possibly-sent": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, `window.__dwFixture.world.coinjoin.drainUnknown = "next"`);
      await expect(page.getByTestId("move-mixed-refusal")).toBeVisible();
    },
  },
  "move-shielded-changed": {
    roots: ["move-mixed-sheet"],
    open: async (page, theme, lang) => {
      await shieldMove(page, theme, lang, `window.__dwFixture.world.coinjoin.drainChanged = true`);
      await expect(page.getByTestId("move-mixed-replanned")).toBeVisible();
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
          panes: state.roots.filter((id) => id.endsWith("-sheet")).map((id) => `[data-testid="${id}"]`),
          data: [...AMOUNTS, ...(state.data ?? [])],
          open: (lang) => state.open(page, theme, lang),
        });
        expect(errors).toEqual([]);
      });

const SETTINGS_ENGLISH = [
  "Mixing rounds",
  "Target balance",
  "Parallel sessions",
  "Inputs per denomination",
  "This setting determines the amount of individual masternodes",
  "This amount acts as a threshold",
  "Use this many separate masternodes",
  "How many inputs of each denominated amount",
];

{
  const lang = "en-XA";
  test(`Settings ▸ CoinJoin's field labels and hints follow ${lang}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await openHome(page, "coinjoin-mixing", "light", lang);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await page.getByTestId("settings-tabs-coinjoin").click();
    const card = page.locator("main.page");
    await expect(page.getByTestId("settings-coinjoin-rounds")).toBeVisible();
    for (const english of SETTINGS_ENGLISH) await expect(card).not.toContainText(english);
    expect(errors).toEqual([]);
  });
}

/** The left edges of `re`'s two groups in the first text node under `root` that matches it. */
async function lefts(page: Page, root: string | Locator, re: RegExp): Promise<[number, number] | null> {
  return (typeof root === "string" ? page.getByTestId(root) : root).evaluate(
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

test.describe("numbers keep their order under ar-XB", () => {
  async function openDetails(page: Page) {
    await openCoinJoin(page, "coinjoin-mixing", "light", "ar-XB");
    await page.getByTestId("coinjoin-details").locator("summary").click();
  }

  test("the session details' amount comes before its unit", async ({ page }) => {
    await openDetails(page);
    const at = await lefts(page, "coinjoin-detail-amount", /(\p{Nd})[\p{Nd}.,]* (t?DASH)/u);
    expect(at, "an amount and its unit").not.toBeNull();
    expect(at![0]).toBeLessThan(at![1]);
  });

  test("the submitted denominations come before their unit", async ({ page }) => {
    await openDetails(page);
    const at = await lefts(page, "coinjoin-detail-denoms", /(\p{Nd})[\p{Nd}.,]* (t?DASH)/u);
    expect(at, "a denomination and its unit").not.toBeNull();
    expect(at![0]).toBeLessThan(at![1]);
  });

  test("the intermediate level's 30 minutes", async ({ page }) => {
    await openCoinJoin(page, "coinjoin-mixing", "light", "ar-XB");
    const at = await lefts(page, "coinjoin-level-intermediate", /(\p{Nd})(\p{Nd})\p{Cf}* minutes/u);
    expect(at, "30 in the intermediate level").not.toBeNull();
    expect(at![0]).toBeLessThan(at![1]);
  });

  test("the combine prompt's limit of 50", async ({ page }) => {
    await openCoinJoin(page, "coinjoin-move-many", "light", "ar-XB");
    await page.getByTestId("coinjoin-move-shield").click();
    await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "combine");
    const at = await lefts(page, "move-mixed-combine-text", /(\p{Nd})(\p{Nd})\p{Cf}*\)/u);
    expect(at, "the (50) limit").not.toBeNull();
    expect(at![0]).toBeLessThan(at![1]);
  });

  for (const mode of ["list", "tree"] as const)
    test(`the coin table's amounts, ${mode}`, async ({ page }) => {
      await page.goto(fixtureUrl("tools-coinjoin", "light", "&lang=ar-XB"));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
      await page.getByTestId("settings-unit-milliDash").click();
      await expect(page.getByTestId("settings-unit-milliDash")).toHaveAttribute("aria-pressed", "true");
      await openToolsTab(page, "coinSelection");
      await page.getByTestId(`coins-mode-${mode}`).click();
      const cell = page.getByTestId("coin-table").locator(mode === "tree" ? "tr.coin-group td.num" : "tbody tr:not(.coin-group) td.num");
      const grouped = cell.filter({ hasText: /\p{Nd}\s\p{Nd}/u }).first();
      const at = await lefts(page, grouped, /(\p{Nd})\p{Nd}*\s(\p{Nd})/u);
      expect(at, "an amount with a digit group").not.toBeNull();
      expect(at![0]).toBeLessThan(at![1]);
    });
});

test("the possibly-sent refusal of a move to Shielded follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await shieldMove(page, "light", "en", `window.__dwFixture.world.coinjoin.drainUnknown = "next"`);
  const refusal = page.getByTestId("move-mixed-refusal");
  await expect(refusal).toHaveText("The payment may have been sent. Open Tools ▸ Repair to settle it; you can move Dash into Shielded again once it has.");
  await page.evaluate(() => window.__dwFixture!.setLanguage("en-XA"));
  await expect(refusal).toHaveText(/^⟦.+⟧$/);
  expect(errors).toEqual([]);
});
