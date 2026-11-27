import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, openToolsTab, trackPageErrors } from "./helpers";
import { expectSaid, liveRegion, liveState, regionWhenAdded } from "./liveRegion";

// A tool's result goes in `Outcome` (src/screens/tools/common.tsx). A live region that appears
// already holding its text is not reliably announced (#314), so each site keeps its empty region
// in the page before the action and that same element is given the result. Its politeness is set
// by what it can show: polite where it says how an action went, assertive where it only says what
// went wrong.

async function open(page: Page, scenario: string, tab?: Parameters<typeof openToolsTab>[1]) {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  if (tab) await openToolsTab(page, tab);
  return errors;
}

async function authorize(page: Page) {
  const sheet = page.getByTestId("grant-sheet");
  await sheet.locator("input[type=password]").fill("demo");
  await sheet.locator("input[type=password]").press("Enter");
  await expect(sheet).toHaveCount(0);
}

const OUTCOME_SITES: {
  where: string;
  region: string;
  politeness: "polite" | "assertive";
  said: string | RegExp;
  reach: (page: Page) => Promise<string[]>;
  act: (page: Page) => Promise<void>;
}[] = [
  {
    where: "Repair ▸ Finish transfers (repair-outcome)",
    region: '[data-testid="repair-outcome"]',
    politeness: "polite",
    said: "0 resumed, 0 completed, 0 still pending.",
    reach: (page) => open(page, "tools-stuck-funding", "repair"),
    act: async (page) => {
      await page.getByTestId("repair-finish-transfers").click();
      await authorize(page);
    },
  },
  {
    where: "Repair ▸ Rescan from a height that is not a whole number (repair-outcome)",
    region: '[data-testid="repair-outcome"]',
    politeness: "polite",
    said: "Enter a block height.",
    reach: (page) => open(page, "funded", "repair"),
    act: async (page) => {
      await page.getByTestId("repair-rescan-height").fill("12.5");
      await page.getByTestId("repair-rescan-from").click();
    },
  },
  {
    where: "Sign message (sign-result)",
    region: '[data-testid="sign-result"]',
    politeness: "polite",
    said: /The entered address is invalid\./,
    reach: (page) => open(page, "funded", "signVerify"),
    act: async (page) => {
      await page.getByTestId("sign-address").fill("nonsense");
      await page.getByTestId("sign-sign").click();
    },
  },
  {
    where: "Verify message (verify-result)",
    region: '[data-testid="verify-result"]',
    politeness: "polite",
    said: "Invalid address: check it and try again.",
    reach: (page) => open(page, "funded", "signVerify"),
    act: async (page) => {
      await page.getByTestId("verify-address").fill("nonsense");
      await page.getByTestId("verify-signature").fill("nonsense");
      await page.getByTestId("verify-verify").click();
    },
  },
  {
    where: "Information ▸ DashPay ▸ Sync now (information-dashpay-outcome)",
    region: '[data-testid="information-dashpay-outcome"]',
    politeness: "polite",
    said: "A DashPay pass was asked for. It runs in the background. Passes don't report their results yet, so this card may not show when it ends.",
    reach: (page) => open(page, "dashpay-info-spv", "information"),
    act: (page) => page.getByTestId("information-dashpay-sync-now").click(),
  },
  {
    where: "Information ▸ DashPay ▸ a refused Sync now (information-dashpay-outcome)",
    region: '[data-testid="information-dashpay-outcome"]',
    politeness: "polite",
    said: /Could not ask for a DashPay pass/,
    reach: async (page) => {
      const errors = await open(page, "dashpay-info-spv", "information");
      await page.evaluate(() => window.__dwFixture!.fail("dashpay_sync_soon", "none"));
      return errors;
    },
    act: (page) => page.getByTestId("information-dashpay-sync-now").click(),
  },
  {
    where: "Address Book ▸ Export (address-book-exported)",
    region: '[data-testid="address-book-exported"]',
    politeness: "polite",
    said: /Sending addresses\.csv/,
    reach: (page) => open(page, "funded", "addressBook"),
    act: (page) => page.getByTestId("address-book-export").click(),
  },
  {
    where: "Address Book ▸ a book that cannot be read again (address-book-problem)",
    region: '[data-testid="tools-addressBook"] [data-testid="address-book-problem"]',
    politeness: "assertive",
    said: "Something went wrong (internal).",
    reach: async (page) => {
      const errors = await open(page, "funded", "addressBook");
      await expect(page.locator("[data-testid^=address-book-delete-]").first()).toBeVisible();
      return errors;
    },
    act: async (page) => {
      await page.evaluate(() => window.__dwFixture!.fail("address_book", "none"));
      await page.locator("[data-testid^=address-book-delete-]").first().click();
    },
  },
];

for (const site of OUTCOME_SITES) {
  test(`${site.where}: the result is said by the region present before the action`, async ({ page }) => {
    const errors = await site.reach(page);
    const region = await liveRegion(page, site.region);
    expect(await region.evaluate(liveState)).toBe(`${site.politeness}: `);
    await site.act(page);
    await expect(page.locator(site.region)).toHaveText(site.said);
    await expectSaid(region, site.politeness, await shown(page, site.region));
    expect(errors).toEqual([]);
  });
}

/** The region's text as a whole, for `expectSaid`. */
async function shown(page: Page, selector: string) {
  return ((await page.locator(selector).textContent()) ?? "").replace(/\s+/g, " ").trim();
}

test("Address Book picker ▸ a book that cannot be read (address-book-problem): the error is said by the region the picker opens with", async ({ page }) => {
  const errors = await open(page, "funded");
  await page.getByTestId("sidebar-pay").click();
  await page.evaluate(() => window.__dwFixture!.fail("address_book", "none"));
  const selector = '[data-testid="address-book-picker"] [data-testid="address-book-problem"]';
  const added = await regionWhenAdded(page, selector);
  await page.getByTestId("pay-address-book").click();
  const { region, added: first } = await added();
  await expect(page.locator(selector)).toHaveText("Something went wrong (internal).");
  expect(first).toMatch(/^(assertive|outside the open modal|not rendered|hidden from assistive tech): $/);
  await expectSaid(region, "assertive", await shown(page, selector));
  expect(errors).toEqual([]);
});
