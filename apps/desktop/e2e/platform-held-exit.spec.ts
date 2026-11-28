import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// The way out of a held Platform item, on the fixture backend: a payment that may still go through
// is moved to the Dash wallet instead (only one of the two goes through), and a held move or contact
// payment is cancelled to the wallet's own Platform address. The rules are unit-tested
// (platform/cancel, fixture/pending).

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";

async function details(page: Page, scenario: string, txid: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
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
      // Only the payment's input, 0.3 of the 0.4, less the move's fee; no line says everything moves.
      await expect(page.getByTestId("platform-move-quote-credits")).toHaveText("0.29998000 tDASH");
      await expect(page.getByText("Everything on your Platform addresses moves to your Dash wallet.", { exact: false })).toHaveCount(0);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`an exit waits for Activity to explain what it replaces (${theme})`, async ({ page }, info) => {
      await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland", theme);
      await page.getByTestId("source-detail-move-to-core").click();
      await page.getByTestId("transfer-continue").click();
      await expect(page.getByTestId("platform-move-go")).toBeEnabled();
      const reads = await calls(page, "platform_activity");
      const activity = await page.evaluate(() => {
        const f = window.__dwFixture!;
        const p = f.world.pending.platform!;
        const rows = p.activity;
        p.activity = rows.filter((a) => a.id !== "ps-held");
        f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "platformAddresses" } });
        return rows;
      });
      await expect.poll(() => calls(page, "platform_activity")).toBeGreaterThan(reads + 1);
      await expect(page.getByTestId("platform-move-quote")).toBeVisible();
      await expect(page.getByTestId("platform-move-go")).toBeDisabled();
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `exit-confirm-checking-${theme}.png`) });
      await expect.soft(page.getByTestId("platform-move-exit-checking")).toHaveText("Checking for an unfinished move…", { timeout: 1500 });
      await expect.soft(page.getByText("Everything on your Platform addresses moves", { exact: false })).toHaveCount(0, { timeout: 1500 });
      await page.evaluate((rows) => {
        const f = window.__dwFixture!;
        f.world.pending.platform!.activity = rows;
        f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "platformAddresses" } });
      }, activity);
      await expect(page.getByTestId("platform-move-exit-send")).toContainText("only one of the two can go through");
      await expect(page.getByTestId("platform-move-go")).toBeEnabled();
    });

    test(`a failed exit waits for its Activity row before naming a winner (${theme})`, async ({ page }, info) => {
      await details(page, "platform-send-failed-may-land", "platform:send:ps-held:mayland", theme, "&hold=platform_move_to_core");
      await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "lost"));
      await page.getByTestId("source-detail-move-to-core").click();
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId("platform-move-go").click();
      await authorize(page);
      await expect.poll(() => calls(page, "platform_move_to_core")).toBe(1);
      const activity = await page.evaluate(() => {
        const p = window.__dwFixture!.world.pending.platform!;
        const rows = p.activity;
        p.activity = rows.filter((a) => a.kind !== "moveToCore");
        window.__dwFixture!.release("platform_move_to_core");
        return rows;
      });
      await expect(page.getByTestId("platform-move-outcome-title")).toBeVisible();
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `exit-result-checking-${theme}.png`) });
      await expect.soft(page.getByTestId("platform-move-outcome-title")).toHaveText("Checking…", { timeout: 1500 });
      await expect.soft(page.getByTestId("platform-move-outcome-text")).not.toContainText("was paid", { timeout: 1500 });
      await page.evaluate((rows) => {
        const f = window.__dwFixture!;
        f.world.pending.platform!.activity = rows;
        f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "platformAddresses" } });
      }, activity);
      await expect(page.getByTestId("platform-move-outcome-text")).toContainText("was paid");
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
    // Focus stays in the sheet, so Escape closes it.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("source-detail")).toHaveCount(0);
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
    await expect(page.getByTestId("source-detail-to").locator("bdi[title]")).toHaveAttribute("title", "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-outcome")).toContainText("Payment cancelled. Bob Builder wasn't paid, and the amount is in your Platform balance, minus the fee.");
  });
});

test.describe("A held cancel, or a held item too small to cancel: Move to Dash wallet", () => {
  const CANCEL = "platform:cancelMove:pc-held:mayland";
  const PAYMENT_CANCEL = "platform:cancelPayment:pc-held:mayland";
  async function moveFromDetails(page: Page) {
    await page.getByTestId("source-detail-move-to-core").click();
    await expect(page.getByTestId("source-detail")).toHaveCount(0);
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "platformToCore");
    await page.getByTestId("transfer-continue").click();
  }

  for (const theme of THEMES) {
    test(`a held cancel's details offer the move, whose Confirm names the cancel and the move it cancels (${theme})`, async ({ page }) => {
      const errors = await details(page, "platform-cancel-failed-may-land", CANCEL, theme);
      await expect(page.getByTestId("source-detail-cancel")).toHaveCount(0);
      await moveFromDetails(page);
      await expect(page.getByTestId("platform-move-exit-send")).toHaveAttribute("data-exit", "cancel");
      await expect(page.getByTestId("platform-move-exit-send")).toHaveText(
        "This cancel of 0.40000000 tDASH, and the move it cancels, may still go through. Moving uses the same Platform funds to send to your Dash wallet instead, so only one of the three can go through. If this move goes through, neither of the others does, and the amount, less fees, goes to your Dash wallet.",
      );
      await expect(page.getByTestId("platform-move-quote")).toBeVisible();
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });

    test(`a held move too small to cancel offers the move instead, whose Confirm names it (${theme})`, async ({ page }) => {
      const errors = await details(page, "platform-move-too-small-may-land", "platform:moveToCore:pm-small:mayland", theme);
      await page.getByTestId("source-detail-cancel").click();
      await expect(page.getByTestId("platform-cancel-quote-error")).toHaveText(
        "This move of 0.00015000 tDASH is too small to cancel. You can move the same funds to your Dash wallet instead. Only one of the two can go through, and either way the amount, less fees, reaches your Dash wallet.",
      );
      await expect(page.getByTestId("platform-cancel-go")).toHaveCount(0);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await moveFromDetails(page);
      await expect(page.getByTestId("platform-move-exit-send")).toHaveText(
        "This move of 0.00015000 tDASH may still go through. Moving again uses the same Platform funds, so only one of the two can go through, and either way the amount, less fees, reaches your Dash wallet.",
      );
      expect(await calls(page, "platform_cancel")).toBe(0);
      expect(errors).toEqual([]);
    });
  }

  test("the move goes through: neither the cancel nor the earlier move did", async ({ page }) => {
    await details(page, "platform-cancel-failed-may-land", CANCEL);
    await moveFromDetails(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("Moved to Dash wallet. The cancel and the earlier move didn't go through.");
    const rows = await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.filter((a) => a.id === "pm-held" || a.id === "pc-held"));
    expect(rows.map((a) => [a.id, a.state, a.mayLand])).toEqual([
      ["pc-held", "failed", false],
      ["pm-held", "failed", false],
    ]);
  });

  test("the cancel went through first: the amount is back on Platform and the move wasn't charged", async ({ page }) => {
    await details(page, "platform-cancel-failed-may-land", CANCEL);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "lostToCancel"));
    await moveFromDetails(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("The move did not go through");
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("The cancel went through before the move, so the amount is in your Platform balance, minus the cancel's fee. The move wasn't charged.");
  });

  test("beside a held payment's cancel: the payment went through first, so Bob was paid", async ({ page }) => {
    await details(page, "platform-payout-cancel-failed-may-land", PAYMENT_CANCEL);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "lost"));
    await moveFromDetails(page);
    await expect(page.getByTestId("platform-move-exit-send")).toContainText("This cancel of 0.04000000 tDASH, and the payment to Bob Builder it cancels, may still go through.");
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("The payment went through before the move, so Bob Builder was paid. The move wasn't charged.");
  });

  test("a held cancel's re-send that Platform's rules refuse offers the move instead", async ({ page }) => {
    await details(page, "platform-cancel-failed-may-land", "platform:moveToCore:pm-held:mayland");
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.cancelPreCheck = "rulesUnsupported"));
    await page.getByTestId("source-detail-cancel").click();
    await page.getByTestId("platform-cancel-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-cancel-refused")).toContainText("Dash Platform's rules have changed");
    await moveFromDetails(page);
    await expect(page.getByTestId("platform-move-exit-send")).toHaveAttribute("data-exit", "cancel");
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("Moved to Dash wallet. The cancel and the earlier move didn't go through.");
  });

  test("a held payment too small to cancel offers the move, which names Bob", async ({ page }) => {
    await details(page, "platform-payout-too-small-may-land", "platform:contactPayment:cp-small:mayland");
    await page.getByTestId("source-detail-cancel").click();
    await expect(page.getByTestId("platform-cancel-quote-error")).toContainText("This payment of 0.00015000 tDASH to Bob Builder is too small to cancel.");
    await moveFromDetails(page);
    await expect(page.getByTestId("platform-move-exit-send")).toContainText("This payment of 0.00015000 tDASH to Bob Builder may still go through.");
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("Moved to Dash wallet. The payment to Bob Builder didn't go through, so Bob Builder wasn't paid.");
  });
});

test.describe("A cancel's result, reopened after Move to Dash wallet went through in its place", () => {
  for (const theme of THEMES) {
    test(`says the move went through and the contact wasn't paid (${theme})`, async ({ page }) => {
      await page.clock.install({ time: FIXTURE_NOW });
      const errors = await details(page, "platform-payout-failed-may-land", "platform:contactPayment:cp-held:mayland", theme);
      await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
      await page.getByTestId("source-detail-cancel").click();
      await page.getByTestId("platform-cancel-go").click();
      await authorize(page);
      await expect(page.getByTestId("platform-cancel-outcome")).toHaveAttribute("data-result", "pending");
      // Leaving the details keeps the cancel's result for when the payment is shown again.
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("source-detail")).toHaveCount(0);
      // Platform answers that the cancel's nonces are not taken: it may still go through, and Activity's next read shows it.
      const cancel = await page.evaluate(() => {
        const p = window.__dwFixture!.world.pending.platform!;
        const c = p.activity.find((a) => a.cancels === "cp-held")!;
        c.state = "failed";
        p.nextMove = "done";
        return c.id;
      });
      await page.clock.runFor(31_000);
      await page.locator(`button[data-txid="platform:cancelPayment:${cancel}:mayland"]`).click();
      await page.getByTestId("source-detail-move-to-core").click();
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId("platform-move-go").click();
      await authorize(page);
      await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("Moved to Dash wallet. The cancel and the payment to Bob Builder didn't go through, so Bob Builder wasn't paid.");
      await page.getByTestId("platform-move-done").click();
      await page.getByTestId("sidebar-activity").click();
      await page.locator('button[data-txid="platform:contactPayment:cp-held"]').click();
      const result = page.getByTestId("platform-cancel-outcome");
      await expect(result).toHaveAttribute("data-result", "exited");
      await expect(result).toContainText("Moved to Dash wallet. The cancel and the payment to Bob Builder didn't go through, so Bob Builder wasn't paid.");
      await expect(result).not.toContainText("Bob Builder was paid");
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
});
