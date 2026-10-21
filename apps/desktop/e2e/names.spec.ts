import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// MG-11: Masternodes ▸ Username voting on the fixture (MG-07's contests and voters): the list with
// search, the pane's tally and voters, the vote sheet and its result with an unconfirmed vote that
// Check again finds, and the states (empty, browsing only, watch-only, unavailable, ending, ended,
// partial), in light and dark with 0 serious axe issues. The clock is pinned to FIXTURE_NOW.

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light", clock: "fixed" | "installed" = "fixed") {
  const errors = trackPageErrors(page);
  if (clock === "fixed") await page.clock.setFixedTime(FIXTURE_NOW);
  else await page.clock.install({ time: FIXTURE_NOW });
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  await page.getByTestId("masternodes-tabs-names").click();
  return errors;
}

for (const theme of THEMES) {
  test(`username voting ${theme}: rows, tally, voters and search; 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "names", theme);
    await expect(page.getByTestId("name-row-a11ce")).toHaveAttribute("aria-current", "true");
    await expect(page.getByTestId("name-row-a11ce")).toContainText("2 contenders");
    await expect(page.getByTestId("name-row-a11ce")).toContainText("You: 2 of 3 voted");
    await expect(page.getByTestId("name-row-a11ce")).toContainText("14 votes");
    await expect(page.getByTestId("name-pane-lead")).toHaveText("8ZtW…R97P leads with 9 of 14 votes");
    await expect(page.getByTestId("name-tally").locator("li")).toHaveCount(4);
    await expect(page.getByTestId("name-pane-masternodes")).toContainText("Votes Lock · No votes left");
    await expect(page.getByTestId("name-pane-masternodes")).toContainText("No vote yet · 5 votes left");
    await expect(page.getByTestId("name-vote")).toBeEnabled();
    // The header's Governance actions belong to the Governance tab.
    await expect(page.getByTestId("governance-create")).toHaveCount(0);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    // DPNS's comparison: "Alice" finds a11ce.
    await page.getByTestId("names-search").locator("input").fill("Alice");
    await expect(page.getByTestId("names-list").locator("li")).toHaveCount(1);
    await page.getByTestId("names-search").locator("input").fill("bob");
    await expect(page.getByTestId("names-no-match")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`username vote sheet ${theme}: voters at their limit off, an unconfirmed vote checked again`, async ({ page }) => {
    const errors = await open(page, "names-vote-mixed", theme);
    await page.getByTestId("name-vote").click();
    const sheet = page.getByTestId("name-vote-sheet");
    await expect(sheet).toBeVisible();
    // No choice is preset; the evonode used its 5 votes on a11ce, the two others start checked.
    await expect(page.getByTestId("name-vote-send")).toBeDisabled();
    await expect(sheet.getByTestId("name-vote-node-b2b2b2b2").locator("input")).toBeDisabled();
    await expect(sheet.getByTestId("name-vote-node-b2b2b2b2")).toContainText("No votes left");
    await expect(sheet.getByTestId("name-vote-node-c3c3c3c3")).toContainText("PoSe banned, still votes on names");
    await expect(page.getByTestId("name-vote-privacy")).toBeVisible();
    await expect(page.getByTestId("name-vote-selection")).toHaveText("2 masternodes · weight 2 · no fee");
    // The present choice is off for the voter already on it.
    await sheet.getByTestId("name-choice-id:8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P").click();
    await expect(sheet.getByTestId("name-vote-node-a1a1a1a1")).toContainText("Votes 8ZtW…R97P already");
    await expect(page.getByTestId("name-vote-selection")).toHaveText("1 masternode · weight 1 · no fee");
    await sheet.getByTestId("name-choice-abstain").click();
    await expect(page.getByTestId("name-vote-send")).toHaveText("Vote Abstain");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("name-vote-passphrase").fill("demo");
    await page.getByTestId("name-vote-send").click();
    await expect(page.getByTestId("name-vote-result-title")).toHaveText("0 of 2 votes counted · 1 not confirmed yet");
    await expect(page.getByTestId("name-vote-result-a1a1a1a1")).toContainText("Unconfirmed");
    await expect(page.getByTestId("name-vote-result-a1a1a1a1")).toContainText("check again in a minute");
    await expect(page.getByTestId("name-vote-result-c3c3c3c3")).toContainText("Platform's masternode list doesn't have it yet.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    // The vote did land: the re-read shows the choice, so it counts.
    await page.getByTestId("name-vote-check").click();
    await expect(page.getByTestId("name-vote-result-title")).toHaveText("1 of 2 votes counted");
    await expect(page.getByTestId("name-vote-result-a1a1a1a1")).toContainText("Platform now shows this choice");
    await expect(page.getByTestId("name-vote-check")).toHaveCount(0);
    await expect(page.getByTestId("name-vote-done")).toBeFocused();
    await page.getByTestId("name-vote-done").click();
    await expect(sheet).toBeHidden();
    expect(errors).toEqual([]);
  });
}

test("a counted vote moves the tally and the row's You:", async ({ page }) => {
  await open(page, "names");
  await page.getByTestId("name-vote").click();
  await page.getByTestId("name-choice-id:4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF").click();
  await page.getByTestId("name-vote-passphrase").fill("demo");
  await page.getByTestId("name-vote-send").click();
  await expect(page.getByTestId("name-vote-result-title")).toHaveText("2 of 2 votes counted");
  await expect(page.getByTestId("name-vote-check")).toHaveCount(0);
  await page.getByTestId("name-vote-done").click();
  await expect(page.getByTestId("name-pane-lead")).toHaveText("8ZtW…R97P leads with 8 of 15 votes");
  await expect(page.getByTestId("name-tally-id:4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF")).toContainText("6");
  await expect(page.getByTestId("name-row-a11ce")).toContainText("You: 3 of 3 voted");
});

test("a Check again that can't read says so and counts nothing; the tab says its votes may be out of date", async ({ page }) => {
  const errors = await open(page, "names-check-fails");
  await page.getByTestId("name-vote").click();
  await page.getByTestId("name-choice-abstain").click();
  await page.getByTestId("name-vote-passphrase").fill("demo");
  await page.getByTestId("name-vote-send").click();
  await expect(page.getByTestId("name-vote-result-title")).toHaveText("1 of 2 votes counted · 1 not confirmed yet");
  await page.getByTestId("name-vote-check").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("name-vote-check-error")).toHaveText("Couldn't check again. Couldn't reach Dash Platform. Try again in a moment.");
  await expect(page.getByRole("status").filter({ hasText: "Couldn't check again." })).toHaveCount(1);
  // The keyboard user is still on Check again.
  await expect(page.getByTestId("name-vote-check")).toBeFocused();
  await expect(page.getByTestId("name-vote-result-title")).toHaveText("1 of 2 votes counted · 1 not confirmed yet");
  await expect(page.getByTestId("name-vote-result-a1a1a1a1")).toContainText("Unconfirmed");
  await page.getByTestId("name-vote-done").click();
  await expect(page.getByTestId("names-stale")).toContainText("These votes may be out of date.");
  await expect(page.getByTestId("name-pane")).toBeVisible();
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("Vote goes off when the contest ends under the open sheet", async ({ page }) => {
  await open(page, "names-ending", "light", "installed");
  await page.getByTestId("name-vote").click();
  await page.getByTestId("name-choice-lock").click();
  await page.getByTestId("name-vote-passphrase").fill("demo");
  await expect(page.getByTestId("name-vote-send")).toBeEnabled();
  await page.clock.setSystemTime(FIXTURE_NOW.getTime() + (3 * 3600 + 1) * 1000);
  await page.clock.runFor(31_000);
  await expect(page.getByTestId("name-vote-closed")).toHaveText("Voting on this name has ended.");
  await expect(page.getByRole("status").filter({ hasText: "Voting on this name has ended." })).toHaveCount(1);
  await expect(page.getByTestId("name-vote-send")).toBeDisabled();
});

test("a wrong passphrase keeps the username vote sheet open and says so", async ({ page }) => {
  await open(page, "names");
  await page.getByTestId("name-vote").click();
  await page.getByTestId("name-choice-lock").click();
  await page.getByTestId("name-vote-passphrase").fill("wrong");
  await page.getByTestId("name-vote-send").click();
  await expect(page.getByTestId("name-vote-error")).toHaveText("The passphrase you entered was incorrect.");
  await expect(page.getByTestId("name-vote-result")).toHaveCount(0);
});

test("the tabs are a tablist: the arrow keys move between them", async ({ page }) => {
  await open(page, "names");
  await expect(page.getByTestId("masternodes-tabs-names")).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("masternodes-tabs-names").press("ArrowLeft");
  await expect(page.getByTestId("masternodes-tabs-governance")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("masternodes-tabs-governance")).toBeFocused();
  await expect(page.getByTestId("governance-create")).toBeVisible();
});

/** Each state's scenario and what it shows. */
const STATES: { scenario: string; check: (page: Page) => Promise<void> }[] = [
  {
    scenario: "names-empty",
    check: async (page) => {
      await expect(page.getByTestId("names-empty")).toContainText("No usernames are being voted on");
    },
  },
  {
    scenario: "names-no-voters",
    check: async (page) => {
      await expect(page.getByTestId("names-browsing-only")).toBeVisible();
      await expect(page.getByTestId("name-vote")).toBeDisabled();
      await expect(page.getByTestId("name-vote-blocked")).toHaveText("No masternode in this wallet can vote on usernames");
    },
  },
  {
    scenario: "names-watch-only",
    check: async (page) => {
      await expect(page.getByTestId("names-notice-watch-only")).toBeVisible();
      await expect(page.getByTestId("name-vote")).toBeDisabled();
    },
  },
  {
    scenario: "names-list-unavailable",
    check: async (page) => {
      await expect(page.getByTestId("names-error")).toContainText("Couldn't read the votes on usernames");
      await expect(page.getByTestId("names-error")).toContainText("The masternode list hasn't loaded yet.");
      await expect(page.getByTestId("names-retry")).toBeEnabled();
    },
  },
  {
    scenario: "names-platform-unavailable",
    check: async (page) => {
      await expect(page.getByTestId("names-error")).toContainText("Couldn't reach Dash Platform. Try again in a moment.");
      await expect(page.getByTestId("names-retry")).toBeEnabled();
    },
  },
  {
    scenario: "names-ending",
    check: async (page) => {
      await expect(page.getByTestId("name-pane-chip")).toHaveText("Ends in 3 hours");
      await expect(page.getByTestId("name-vote")).toBeEnabled();
    },
  },
  {
    scenario: "names-ended",
    check: async (page) => {
      await expect(page.getByTestId("name-pane-chip")).toHaveText("Voting ended");
      await expect(page.getByTestId("name-closed")).toBeVisible();
      await expect(page.getByTestId("name-vote")).toBeDisabled();
      await expect(page.getByTestId("name-vote-blocked")).toHaveText("Voting on this name has ended");
    },
  },
  {
    scenario: "names-partial",
    check: async (page) => {
      await expect(page.getByTestId("name-partial")).toContainText("first 100 contenders");
      await expect(page.getByTestId("name-row-a11ce")).toContainText("2+ contenders");
    },
  },
];

for (const theme of THEMES) {
  for (const { scenario, check } of STATES) {
    test(`${scenario} ${theme}: 0 serious axe issues`, async ({ page }) => {
      const errors = await open(page, scenario, theme);
      await check(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
