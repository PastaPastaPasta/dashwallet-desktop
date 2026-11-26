import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// The way out of a held Platform item, on the fixture backend: a payment that may still go through
// is moved to the Dash wallet instead (only one of the two goes through), and a held move or contact
// payment is cancelled to the wallet's own Platform address. The rules are unit-tested
// (platform/cancel, fixture/pending).

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";

async function details(page: Page, scenario: string, txid: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  await page.locator(`button[data-txid="${txid}"]`).click();
  await page.getByTestId("source-detail").waitFor();
  return errors;
}

const calls = (page: Page, name: string) => page.evaluate((n) => window.__dwFixture!.calls.filter((c) => c === n).length, name);

test.describe("A held payment from Platform: Move to Dash wallet", () => {
  for (const theme of THEMES) {
    test(`its details offer the move, whose Confirm names the payment it replaces (${theme})`, async ({ page }) => {
      const errors = await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland", theme);
      await expect(page.getByTestId("source-detail-status")).toHaveText("May still go through");
      await expect(page.getByTestId("source-detail-cancel")).toHaveCount(0);
      await page.getByTestId("source-detail-move-to-core").click();
      await expect(page.getByTestId("source-detail")).toHaveCount(0);
      const body = page.getByTestId("transfer-body");
      await expect(body).toHaveAttribute("data-route", "platformToCore");
      await page.getByTestId("transfer-continue").click();
      await expect(page.getByTestId("platform-move-exit-send")).toHaveText(
        `This payment of 0.01000000 tDASH to tdash1kq7r3m…8p0s2a4d6f8h may still go through. Moving uses the same Platform funds to send to your Dash wallet instead, so only one of the two can go through. If the move goes through, tdash1kq7r3m…8p0s2a4d6f8h isn't paid, and the amount, less fees, goes to your Dash wallet.`,
      );
      // Only the payment's input: 0.3 of the 0.4.
      await expect(page.getByTestId("platform-move-quote-credits")).toContainText("0.3");
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test("the move goes through: the payment didn't, and its recipient wasn't paid", async ({ page }) => {
    await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland");
    await page.getByTestId("source-detail-move-to-core").click();
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("Moved to Dash wallet. The payment to tdash1kq7r3m…8p0s2a4d6f8h didn't go through, so tdash1kq7r3m…8p0s2a4d6f8h wasn't paid.");
    expect(await calls(page, "platform_move_to_core")).toBe(1);
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.find((a) => a.id === "ps-held"))).toMatchObject({ state: "failed", mayLand: false });
  });

  test("the payment went through first: its recipient was paid and the move wasn't charged", async ({ page }) => {
    await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland");
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "lost"));
    await page.getByTestId("source-detail-move-to-core").click();
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("The move did not go through");
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("The payment went through before the move, so tdash1kq7r3m…8p0s2a4d6f8h was paid. The move wasn't charged.");
  });

  test("a plain move beside it is refused before any passphrase; Pay's retry stays offered", async ({ page }) => {
    await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland");
    await page.keyboard.press("Escape");
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await expect(page.getByTestId("platform-send-earlier")).toContainText("may still go through");
    await expect(page.getByTestId("platform-send-retry-held")).toBeVisible();
  });
});

test.describe("A held move or contact payment from Platform: Cancel", () => {
  for (const theme of THEMES) {
    test(`a held move's Cancel names its amount and fee, and Keep waiting runs nothing (${theme})`, async ({ page }) => {
      const errors = await details(page, "platform-move-failed-may-land", "platform:moveToCore:pm-held:mayland", theme);
      await expect(page.getByTestId("source-detail-move-to-core")).toHaveCount(0);
      await page.getByTestId("source-detail-cancel").click();
      await expect(page.getByTestId("platform-cancel-confirm")).toContainText("Cancel this move?");
      await expect(page.getByTestId("platform-cancel-body")).toHaveText(
        "Dash Wallet can't tell yet whether this move of 0.40000000 tDASH to your Dash wallet will go through. Cancelling uses the same Platform funds to send to your own Platform address instead, so only one of the two can go through. If the cancel goes through, the amount stays in your Platform balance, minus a fee of about 0.00006500 tDASH.",
      );
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.getByTestId("platform-cancel-dismiss").click();
      await expect(page.getByTestId("platform-cancel-confirm")).toHaveCount(0);
      await expect(page.getByTestId("source-detail-cancel")).toBeVisible();
      expect(await calls(page, "platform_cancel")).toBe(0);
      expect(errors).toEqual([]);
    });
  }

  test("a cancel that goes through keeps the amount on Platform, and Activity names it", async ({ page }) => {
    await details(page, "platform-move-failed-may-land", "platform:moveToCore:pm-held:mayland");
    await page.getByTestId("source-detail-cancel").click();
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-outcome")).toHaveAttribute("data-result", "done");
    await expect(page.getByTestId("platform-cancel-outcome")).toContainText("Move cancelled. The amount is in your Platform balance, minus the fee.");
    expect(await calls(page, "platform_cancel")).toBe(1);
    await page.getByTestId("platform-cancel-close").click();
    await page.keyboard.press("Escape");
    await expect(page.getByText("Cancel of an earlier move")).toBeVisible();
  });

  test("the move went through first: the cancel says so and wasn't charged", async ({ page }) => {
    await details(page, "platform-move-failed-may-land", "platform:moveToCore:pm-held:mayland");
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "lost"));
    await page.getByTestId("source-detail-cancel").click();
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-outcome")).toHaveAttribute("data-result", "lost");
    await expect(page.getByTestId("platform-cancel-outcome")).toContainText("The move went through before the cancel, so the amount is in your Dash wallet. The cancel wasn't charged.");
  });

  test("an unanswered cancel says it waits for Platform", async ({ page }) => {
    await details(page, "platform-move-failed-may-land", "platform:moveToCore:pm-held:mayland");
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await page.getByTestId("source-detail-cancel").click();
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-outcome")).toHaveAttribute("data-result", "pending");
    await expect(page.getByTestId("platform-cancel-outcome")).toContainText("Cancelling. Dash Wallet is waiting for Dash Platform to confirm which one went through.");
  });

  test("a held contact payment's Cancel names the contact, and the cancel pays no one", async ({ page }) => {
    await details(page, "platform-payout-failed-may-land", "platform:contactPayment:cp-held:mayland");
    await page.getByTestId("source-detail-cancel").click();
    await expect(page.getByTestId("platform-cancel-confirm")).toContainText("Cancel this payment?");
    await expect(page.getByTestId("platform-cancel-body")).toContainText("this payment of 0.04000000 tDASH to Bob Builder will go through");
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-outcome")).toContainText("Payment cancelled. Bob Builder wasn't paid, and the amount is in your Platform balance, minus the fee.");
  });
});
