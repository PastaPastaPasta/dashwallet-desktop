import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, counted, PSEUDO, readFirst } from "./pseudo";

// Localization (MP-14): Masternodes ▸ Username voting under the two pseudo-locales (./pseudo.ts): the
// list, search and sort, the pane's tally and voters, the vote sheet and its result, and the tab's
// states, reached as names.spec.ts reaches them.

/** Counts, the fixture's usernames, identities, masternode names, dates and times. */
const DATA = [
  /^[+−-]?[\p{Nd}.,٫٬\s]+$/u,
  /^⁨?(a11ce|dash-pay)⁩?$/,
  /^⁨?[1-9A-HJ-NP-Za-km-z]{4}…[1-9A-HJ-NP-Za-km-z]{4}⁩?$/,
  /^⁨?[A-Z][a-z]{2} \d{1,2}(, \d{4})?⁩?$/,
  /^⁨?\d{1,2}:\d{2}\s?[AP]M⁩?$/,
  /^bob$/,
];

const VOTER_A = "a1".repeat(32);

const open = async (page: Page, scenario: string, theme: Theme, lang: string, prep?: (page: Page) => Promise<void>) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
  await prep?.(page);
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  await page.getByTestId("masternodes-tabs-names").click();
};

const sheet = async (page: Page) => {
  await page.getByTestId("name-vote").click();
  await expect(page.getByTestId("name-vote-sheet")).toBeVisible();
};

const send = async (page: Page, choice = "abstain") => {
  await sheet(page);
  await page.getByTestId(`name-choice-${choice}`).click();
  await page.getByTestId("name-vote-passphrase").fill("demo");
  await page.getByTestId("name-vote-send").click();
  await expect(page.getByTestId("name-vote-result")).toBeVisible();
};

/** VOTER_A down to its last vote on a11ce, so the sheet warns that it is the last. */
const lastVote = (page: Page) =>
  page.evaluate((a) => {
    const v = window.__dwFixture!.world.governance.names.names.voters.find((x) => x.proTxHash === a)!;
    v.current[0].votesUsed = 4;
  }, VOTER_A);

interface State {
  scenario: string;
  roots: string[];
  prep?: (page: Page) => Promise<void>;
  go?: (page: Page) => Promise<void>;
}

const PAGE = ["page-masternodes"];
const SHEET = ["name-vote-sheet"];

export const STATES: Record<string, State> = {
  list: { scenario: "names", roots: PAGE, go: (p) => expect(p.getByTestId("name-pane-lead")).toBeVisible() },
  "no-match": {
    scenario: "names",
    roots: PAGE,
    go: async (p) => {
      await p.getByTestId("names-search").locator("input").fill("bob");
      await expect(p.getByTestId("names-no-match")).toBeVisible();
    },
  },
  "vote-sheet": {
    scenario: "names-vote-mixed",
    roots: SHEET,
    go: async (p) => {
      await sheet(p);
      await p.getByTestId("name-choice-id:8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P").click();
      await expect(p.getByTestId("name-vote-node-a1a1a1a1")).toContainText(/\S/);
    },
  },
  "last-vote": {
    scenario: "names",
    roots: SHEET,
    prep: lastVote,
    go: async (p) => {
      await sheet(p);
      await p.getByTestId("name-choice-lock").click();
      await expect(p.getByTestId("name-vote-last")).toBeVisible();
    },
  },
  result: { scenario: "names-vote-mixed", roots: SHEET, go: (p) => send(p) },
  "check-fails": {
    scenario: "names-check-fails",
    roots: SHEET,
    go: async (p) => {
      await send(p);
      await p.getByTestId("name-vote-check").click();
      await expect(p.getByTestId("name-vote-check-error")).toBeVisible();
    },
  },
  stale: {
    scenario: "names-check-fails",
    roots: PAGE,
    go: async (p) => {
      await send(p);
      await p.getByTestId("name-vote-check").click();
      await expect(p.getByTestId("name-vote-check-error")).toBeVisible();
      await p.getByTestId("name-vote-done").click();
      await expect(p.getByTestId("names-stale")).toBeVisible();
    },
  },
  still: {
    scenario: "names-still-unconfirmed",
    roots: SHEET,
    go: async (p) => {
      await send(p);
      await p.getByTestId("name-vote-check").click();
      await expect(p.getByTestId("name-vote-check-note")).toBeVisible();
    },
  },
  empty: { scenario: "names-empty", roots: PAGE, go: (p) => expect(p.getByTestId("names-empty")).toBeVisible() },
  "no-voters": { scenario: "names-no-voters", roots: PAGE, go: (p) => expect(p.getByTestId("names-browsing-only")).toBeVisible() },
  unavailable: { scenario: "names-platform-unavailable", roots: PAGE, go: (p) => expect(p.getByTestId("names-error")).toBeVisible() },
  "watch-only": { scenario: "names-watch-only", roots: PAGE, go: (p) => expect(p.getByTestId("names-notice-watch-only")).toBeVisible() },
  ending: { scenario: "names-ending", roots: PAGE, go: (p) => expect(p.getByTestId("name-pane-chip")).toBeVisible() },
  ended: { scenario: "names-ended", roots: PAGE, go: (p) => expect(p.getByTestId("name-closed")).toBeVisible() },
  partial: { scenario: "names-partial", roots: PAGE, go: (p) => expect(p.getByTestId("name-partial")).toBeVisible() },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, s] of Object.entries(STATES))
      test(`Username voting ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `names-${name}`,
          roots: s.roots,
          data: DATA,
          open: async (lang) => {
            await open(page, s.scenario, theme, lang, s.prep);
            await s.go?.(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("Username voting follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "names", "light", "en");
  await expect(page.getByTestId("name-pane-lead")).toHaveText("Alice (8ZtW…R97P) leads with 9 of 18 votes");
  await page.evaluate(() => window.__dwFixture!.setLanguage("en-XA"));
  for (const id of ["names-intro", "name-pane-lead", "name-vote"]) await expect(page.getByTestId(id)).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("names-sort").locator("option").first()).toHaveText(/^⟦.+⟧$/);
  expect(errors).toEqual([]);
});

test("the open vote sheet follows a language change and keeps the choice and the checked masternodes", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "names-vote-mixed", "light", "en");
  await sheet(page);
  await page.getByTestId("name-choice-abstain").click();
  const box = page.getByTestId("name-vote-nodes").locator("input[type=checkbox]:enabled").first();
  await box.uncheck();
  const sub = page.getByTestId("name-vote-nodes").locator(".mg-node-sub").first();
  await expect(sub).not.toHaveText(/⟦/);
  await page.evaluate(() => window.__dwFixture!.setLanguage("en-XA"));
  await expect(sub).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("name-choice-abstain").locator("input")).toBeChecked();
  await expect(box).not.toBeChecked();
  expect(errors).toEqual([]);
});

test.describe("counts keep their order under ar-XB", () => {
  test("a row's votes and contenders, and a voter's votes left, are read number first", async ({ page }) => {
    await open(page, "names", "light", "ar-XB");
    const row = page.getByTestId("name-row-a11ce");
    await readFirst(row.locator(".mg-row-meta"), "a row's votes", counted("votes", "setov"));
    await readFirst(row.locator(".mg-row-sub"), "a row's contenders", counted("contenders", "srednetnoc"));
  });

  test("the vote sheet's choices and the result's title are read number first", async ({ page }) => {
    await open(page, "names-vote-mixed", "light", "ar-XB");
    await sheet(page);
    await readFirst(page.getByTestId("name-choice-lock"), "a choice's votes", counted("votes", "setov"));
    await page.getByTestId("name-choice-abstain").click();
    await page.getByTestId("name-vote-passphrase").fill("demo");
    await page.getByTestId("name-vote-send").click();
    await readFirst(page.getByTestId("name-vote-result-title"), "the result's count", counted("votes", "setov"));
  });
});
