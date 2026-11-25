import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors } from "./helpers";

const txid = (i: number) => (0xd0 + i).toString(16).repeat(32);
const row = (page: Page, i: number) => page.locator(`button.tx-row[data-txid="${txid(i)}"]`);

async function openActivity(page: Page) {
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
}

test.describe("Dropped payments", () => {
  for (const theme of THEMES) {
    test(`Home names a payment dropped for a never-valid source, and its details say why (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("dropped-one", theme));
      const notice = page.getByTestId("dropped-notice");
      await expect(notice).toBeVisible();
      await expect(page.getByTestId("dropped-notice-text")).toContainText("A payment of 0.25");
      await expect(page.getByTestId("dropped-notice-text")).toContainText("didn't go through. The Dash it used came from an incoming transaction that was never valid, so Dash Wallet dropped the payment.");
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.getByTestId("dropped-notice-details").click();
      await expect(page.getByTestId("tx-detail")).toBeVisible();
      await expect(page.getByTestId("tx-detail-status")).toContainText("Dropped");
      await expect(page.getByTestId("tx-detail-dropped")).toContainText("This payment to");
      await expect(page.getByTestId("tx-detail-dropped")).toContainText("If");
      await expect(page.getByTestId("tx-resend")).toHaveCount(0);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("tx-detail")).toHaveCount(0);
      await page.getByTestId("dropped-notice-dismiss").click();
      await expect(notice).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

  test("Home counts several dropped payments, and Details opens Activity", async ({ page }) => {
    await page.goto(fixtureUrl("dropped-several", "light"));
    await expect(page.getByTestId("dropped-notice-text")).toHaveText(
      "2 payments didn't go through. The Dash they used came from an incoming transaction that was never valid, so Dash Wallet dropped them.",
    );
    await page.getByTestId("dropped-notice-details").click();
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await expect(row(page, 0)).toContainText("Dropped");
    await expect(row(page, 1)).toContainText("Dropped");
  });

  test("the notice hides the amount in discreet mode", async ({ page }) => {
    await page.goto(fixtureUrl("dropped-one", "light"));
    await expect(page.getByTestId("dropped-notice-text")).toContainText("0.25");
    await page.getByTestId("hero-amount").click();
    await expect(page.getByTestId("dropped-notice-text")).not.toContainText("0.25");
  });

  for (const theme of THEMES) {
    test(`each state the engine gives has its own line, and no notice until the coins are back (${theme})`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("dropped-states", theme));
      await expect(page.getByTestId("hero-amount")).toBeVisible();
      await expect(page.getByTestId("dropped-notice")).toHaveCount(0);
      await openActivity(page);
      const lines: [number, string][] = [
        [0, "when the current rescan finishes."],
        [1, "open Tools and run Rescan from the wallet's birth."],
        [2, "open Tools and set Wallet birth height to 2103500 or lower."],
        [3, "came from a transaction that hasn't confirmed yet."],
        [4, "open What can I do? under that amount on Home."],
        [5, "because it was never valid, so it can never confirm. It no longer counts toward your balance."],
      ];
      for (const [i, line] of lines) {
        await expect(row(page, i)).toContainText("Dropped");
        await row(page, i).click();
        await expect(page.getByTestId("tx-detail-status")).toContainText("Dropped");
        await expect(page.getByTestId("tx-detail-dropped")).toContainText(line);
        await expect(page.getByTestId("tx-resend")).toHaveCount(0);
        if (i === 0) expect(await seriousAxeViolations(page)).toEqual([]);
        await page.keyboard.press("Escape");
        await expect(page.getByTestId("tx-detail")).toHaveCount(0);
      }
      expect(errors).toEqual([]);
    });
  }
});
