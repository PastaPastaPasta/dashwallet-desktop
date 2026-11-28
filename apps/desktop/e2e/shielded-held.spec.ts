import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, openTransfer, THEMES, trackPageErrors, type Theme } from "./helpers";

// Issue 809: a Shielded payment Platform refused at broadcast may still go through, so its notes
// stay held, out of the balance. Home's Shielded row, Send from Shielded and Activity say so, and
// nothing offers "Try again" for it.

async function shot(page: Page, name: string) {
  const dir = process.env.DW_SHOTS;
  if (dir) {
    await fontsReady(page);
    await page.screenshot({ path: `${dir}/${name}.png` });
  }
}

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await expect(page.getByTestId("hero-breakdown")).toBeVisible();
  return errors;
}

const heldRow = (page: Page, n: number) => page.locator(`button.tx-row[data-txid="shielded:sent:sh-held-${n}:mayland"]`);

test.describe("Shielded funds held for a payment that may still go through", () => {
  for (const theme of THEMES) {
    test(`Home, Send from Shielded and Activity explain one hold (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-held", theme);
      const row = page.getByTestId("breakdown-shielded");
      await expect(row.locator(".breakdown-subtitle")).toBeEmpty();
      await expect(row).toHaveAttribute("tabindex", "0");
      await row.focus();
      await expect(row.getByRole("tooltip")).toContainText("held for a payment");
      await shot(page, `${theme}-home-held-one`);

      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("shielded-send").click();
      const sheet = page.getByTestId("shielded-sheet");
      await expect(sheet).toBeVisible();
      await expect(page.getByTestId("shielded-held")).toContainText(
        "is held for a payment that may still go through. If the network shows it can no longer go through, the amount comes back to your Shielded balance. Dash Wallet keeps checking while it's open.",
      );
      await expect(sheet).not.toContainText(/try again/i);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await shot(page, `${theme}-send-held-one`);
      await page.keyboard.press("Escape");

      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      await expect(heldRow(page, 1)).toContainText("May still go through");
      await expect(heldRow(page, 1)).not.toContainText("Not accepted");
      await shot(page, `${theme}-activity-held`);
      await heldRow(page, 1).click();
      await expect(page.getByTestId("source-detail-status")).toHaveText("May still go through");
      await expect(page.getByTestId("source-detail")).not.toContainText(/try again/i);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await shot(page, `${theme}-activity-held-detail`);
      expect(errors).toEqual([]);
    });

    test(`two holds are counted (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-held-two", theme);
      const row = page.getByTestId("breakdown-shielded");
      await expect(row).toHaveAttribute("tabindex", "0");
      await row.focus();
      await expect(page.getByRole("tooltip")).toContainText("held for 2 payments");
      await shot(page, `${theme}-home-held-two`);
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("shielded-send").click();
      await expect(page.getByTestId("shielded-held")).toContainText("is held for 2 payments that may still go through. If the network shows one can no longer go through, its part comes back to your Shielded balance.");
      await shot(page, `${theme}-send-held-two`);
      expect(errors).toEqual([]);
    });
  }

  test("nothing held: the Shielded row and the sheet are as before", async ({ page }) => {
    await open(page, "funded", "light");
    await expect(page.getByTestId("breakdown-shielded").locator("[role=tooltip]")).toHaveCount(0);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("shielded-send").click();
    await expect(page.getByTestId("shielded-sheet")).toBeVisible();
    await expect(page.getByTestId("shielded-held")).toHaveCount(0);
  });

  test("discreet mode keeps the held amount to itself", async ({ page }) => {
    await open(page, "shielded-held", "light");
    await page.getByTestId("hero-amount").click();
    await expect(page.getByTestId("breakdown-shielded").locator("[role=tooltip]")).toHaveCount(0);
    await expect(page.getByTestId("breakdown-shielded")).not.toContainText("held");
  });

  test("the hold's line clears once the hold ends, with no restart", async ({ page }) => {
    await open(page, "shielded-held", "light");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("held for a payment");
    // A scan that sees the held notes spent: the spend is still listed, holding nothing.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const s = f.world.pending.shielded!;
      s.held = s.held!.map((h) => ({ ...h, credits: "0" }));
      s.heldCredits = "0";
      f.emit({ kind: "platform", network: "testnet", walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } });
    });
    await expect(page.getByTestId("breakdown-shielded").locator("[role=tooltip]")).toHaveCount(0);
  });

  for (const theme of THEMES) {
    test(`a held pool-floor refusal explains the pool floor and hold instead of generic pending (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await page.evaluate(() => {
        window.__dwFixture!.world.pending.shielded!.poolTooSmallHeld = true;
      });
      await openTransfer(page, "shieldedToCore");
      await page.getByTestId("transfer-amount").fill("0.1");
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId("shielded-go").click();
      await authorize(page);
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Shielded pool too small");
      await expect(page.getByTestId("shielded-outcome-text")).toContainText("Shielded withdrawals open once the network's shielded pool has enough activity.");
      await expect(page.getByTestId("shielded-outcome-text")).toContainText("stays held until Dash Wallet proves the move can no longer go through");
      await expect(page.getByTestId("shielded-outcome-title")).not.toHaveText("Not confirmed yet");
      expect(await seriousAxeViolations(page)).toEqual([]);
      await shot(page, `${theme}-shielded-pool-floor-held`);
      await page.getByTestId("shielded-done").click();
      await expect(page.getByTestId("breakdown-shielded")).toContainText("held for a payment");
      expect(errors).toEqual([]);
    });
  }
});
