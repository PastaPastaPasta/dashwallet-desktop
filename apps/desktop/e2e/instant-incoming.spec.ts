// Instant incoming payments (PRODUCT.md #1): a payment that arrives while the wallet still syncs
// shows on Home and in Activity with the balance it makes, and Home says the balance may be short.

import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

const ROW = "tx-row-5f0c2a6e9d-0";
const SYNC_LINE = "Still syncing, balance may be incomplete";

async function checked(page: Page, errors: string[]) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
}

for (const theme of THEMES) {
  for (const [scenario, chip] of [
    ["catching-up-payment", "InstantSend"],
    ["catching-up-pending-payment", "Pending"],
  ] as const) {
    test(`${scenario} (${theme}): Home and Activity show the payment and its balance while syncing`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl(scenario, theme));
      const hero = page.getByTestId("hero");
      await hero.waitFor();
      await expect(hero.getByTestId("hero-amount")).toContainText("26.65", { timeout: 15_000 });
      await expect(hero.getByTestId("breakdown-core")).toContainText("25.00");
      await expect(hero).not.toContainText("Balance unavailable");
      const line = page.getByRole("status").filter({ hasText: SYNC_LINE });
      await expect(line).toHaveCount(1);
      await expect(page.getByTestId("hero-sync-line")).toHaveText(SYNC_LINE);
      const homeRow = page.getByTestId(ROW);
      await expect(homeRow).toContainText("Received");
      await expect(homeRow).toContainText("+25.00");
      await expect(homeRow).toContainText(chip);
      await checked(page, errors);

      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      const row = page.getByTestId(ROW);
      await expect(row).toContainText("+25.00");
      await expect(row).toContainText(chip);
      await checked(page, errors);
    });
  }

  test(`a synced wallet's Home has no sync line (${theme})`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("funded", theme));
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("hero-sync-line")).toHaveText("");
    await expect(page.getByTestId("hero-amount")).not.toContainText("Balance unavailable");
    await checked(page, errors);
  });
}
