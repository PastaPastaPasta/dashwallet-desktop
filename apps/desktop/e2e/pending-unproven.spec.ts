import { expect, test, type Page } from "@playwright/test";
import { en } from "../src/i18n/en";
import { seriousAxeViolations } from "./axe";
import { PAYEE } from "./wallet";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// A coin withheld while the payment that sent it may never confirm: Home shows it pending, Coin
// Selection says why it can't be picked, and the payment's details show the line and Abandon's hint.

const TXID = "e7".repeat(32);
const PENDING = en.coins.pendingUnproven;
const HINT = en.activity.abandon.unprovenParentHint;

async function shot(page: Page, name: string, theme: Theme) {
  const dir = process.env.DW_PU_SHOTS;
  if (!dir) return;
  await fontsReady(page);
  await page.screenshot({ path: `${dir}/${name}-${theme}.png` });
}

test.describe("A withheld coin", () => {
  for (const theme of THEMES) {
    test(`Home shows it pending, Coin Selection says why, and the payment's details explain it (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("pending-unproven", theme));
      const hero = page.getByTestId("hero");
      await expect(hero).toContainText("pending");
      await shot(page, "home", theme);

      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("pay-tabs-address").click();
      await page.getByTestId("pay-address").fill(PAYEE);
      await page.getByTestId("pay-amount").fill("0.1");
      await page.getByTestId("pay-advanced").locator("summary").click();
      await page.getByTestId("coin-choice-selected").check();
      const coin = page.getByTestId(`coin-${TXID.slice(0, 8)}-1`);
      await expect(coin).toContainText(PENDING);
      await expect(coin.locator("input")).toBeDisabled();
      expect(await seriousAxeViolations(page)).toEqual([]);
      await coin.scrollIntoViewIfNeeded();
      await shot(page, "coin-selection", theme);

      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      await page.locator(`button.tx-row[data-txid="${TXID}"]`).click();
      await expect(page.getByTestId("tx-detail-pending-unproven")).toHaveText(PENDING);
      await expect(page.getByTestId("tx-abandon-unproven-hint")).toHaveText(HINT);
      await expect(page.getByTestId("tx-abandon")).toBeVisible();
      expect(await seriousAxeViolations(page)).toEqual([]);
      await shot(page, "tx-detail", theme);

      await page.getByTestId("tx-abandon").click();
      await expect(page.getByTestId("tx-abandon-unproven-hint")).toHaveCount(0);
      await page.getByTestId("tx-abandon-confirm").click();
      await expect(page.getByTestId("tx-detail-pending-unproven")).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

  test("another payment's details show neither line", async ({ page }) => {
    await page.goto(fixtureUrl("pending-unproven", "light"));
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await page.locator(`button.tx-row:not([data-txid="${TXID}"])`).first().click();
    await page.getByTestId("tx-detail-facts").waitFor();
    await expect(page.getByTestId("tx-detail-pending-unproven")).toHaveCount(0);
    await expect(page.getByTestId("tx-abandon-unproven-hint")).toHaveCount(0);
  });
});
