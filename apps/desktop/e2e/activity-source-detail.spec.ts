import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

async function activity(page: Page, scenario = "funded", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  return errors;
}

test.describe("Activity: details of a Shielded or Platform-address row", () => {
  for (const theme of THEMES) {
    test(`a received Shielded payment opens its details with the memo (${theme})`, async ({ page }) => {
      const errors = await activity(page, "funded", theme);
      const row = page.locator('button[data-txid="shielded:received:sh-3"]');
      await row.click();
      const sheet = page.getByTestId("source-detail");
      await expect(sheet).toBeVisible();
      await expect(page.getByTestId("tx-detail")).toHaveCount(0);
      await expect(page.getByTestId("source-detail-title")).toHaveText("Received in Shielded");
      await expect(page.getByTestId("source-detail-amount")).toContainText("0.5");
      await expect(page.getByTestId("source-detail-status")).toHaveText("Confirmed");
      await expect(page.getByTestId("source-detail-when")).not.toBeEmpty();
      await expect(page.getByTestId("source-detail-memo")).toContainText("Memo");
      await expect(page.getByTestId("source-detail-memo")).toContainText("Lunch");
      await expect(page.getByTestId("source-detail-block")).toContainText("1,200,100");
      await expect(page.getByTestId("source-detail-counterparty")).toHaveCount(0);
      await expect(sheet.getByRole("alert")).toHaveCount(0);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(sheet).toHaveCount(0);
      await expect(row).toBeFocused();
      expect(errors).toEqual([]);
    });
  }

  test("a sent Shielded payment shows where it went and its fee, and no memo it does not have", async ({ page }) => {
    await activity(page);
    await page.locator('button[data-txid="shielded:sent:sh-2"]').click();
    await expect(page.getByTestId("source-detail-title")).toHaveText("Sent from Shielded");
    await expect(page.getByTestId("source-detail-amount")).toContainText("-0.1");
    await expect(page.getByTestId("source-detail-counterparty")).toContainText("To");
    await expect(page.getByTestId("source-detail-counterparty")).toContainText("tdash1zxyz");
    await expect(page.getByTestId("source-detail-counterparty").getByRole("button", { name: "Copy address" })).toBeVisible();
    await expect(page.getByTestId("source-detail-fee")).toContainText("0.0002");
    await expect(page.getByTestId("source-detail-memo")).toHaveCount(0);
  });

  test("a Platform-address receipt shows the address it changed", async ({ page }) => {
    await activity(page);
    await page.locator('button[data-txid="platform:incoming:pa-2"]').click();
    await expect(page.getByTestId("source-detail-title")).toHaveText("Received on Platform");
    await expect(page.getByTestId("source-detail-amount")).toContainText("0.1");
    await expect(page.getByTestId("source-detail-status")).toHaveText("Confirmed");
    await expect(page.getByTestId("source-detail-address")).toContainText("Platform address");
    await expect(page.getByTestId("source-detail-address")).toContainText("tdash1q8u4e2c7y5v9nmp3dfz0ht6l2wrk8s4xj7ga5c9e3v0u6y2pnt4qk5m8zr");
    await expect(page.getByTestId("source-detail-block")).toHaveCount(0);
  });

  test("a move to the Dash wallet shows its Core address and fee; the one it replaced has no amount", async ({ page }) => {
    await activity(page, "platform-replaced");
    await page.locator('button[data-txid="platform:moveToCore:pm-retry"]').click();
    await expect(page.getByTestId("source-detail-title")).toHaveText("Moved to Dash wallet");
    await expect(page.getByTestId("source-detail-amount")).toContainText("0.29998");
    await expect(page.getByTestId("source-detail-to")).toContainText("tdash1");
    await expect(page.getByTestId("source-detail-fee")).toContainText("0.00002");
    await page.keyboard.press("Escape");

    await page.locator('button[data-txid="platform:moveToCore:pm-old:replaced"]').click();
    await expect(page.getByTestId("source-detail-title")).toHaveText("Moved to Dash wallet");
    await expect(page.getByTestId("source-detail-status")).toHaveText("Replaced by a retry");
    await expect(page.getByTestId("source-detail-status")).not.toHaveClass(/is-problem/);
    await expect(page.getByTestId("source-detail-amount")).toHaveCount(0);
  });

  for (const theme of THEMES) {
    test(`a group that names no winner says so on every row, its debit on the newest only (${theme})`, async ({ page }) => {
      const errors = await activity(page, "platform-settled-unknown", theme);
      const settled = "Settled with another payment or move";
      const text = "This move and another payment or move from the same Platform addresses couldn't both go through, and only one did. DashPay Desktop can't tell which. Check your Platform balance and Dash wallet before moving again.";
      await expect(page.locator('button[data-txid$=":unknown"]')).toHaveCount(4);
      for (const id of ["pm-exit2", "pm-exit1", "pm-move", "pm-old"]) {
        const row = page.locator(`button[data-txid="platform:moveToCore:${id}:unknown"]`);
        await expect(row).toContainText(settled);
        await expect(row).not.toContainText("Replaced by a retry");
        await expect(row).not.toContainText("Not accepted");
      }
      for (const [id, shown] of [["pm-exit2", 1], ["pm-exit1", 0], ["pm-move", 1], ["pm-old", 0]] as const)
        await expect(page.locator(`button[data-txid="platform:moveToCore:${id}:unknown"] .tx-row-amount`)).toHaveCount(shown);
      // The top-up that did not land is not part of the doubt.
      await expect(page.locator('button[data-txid="platform:toIdentity:pt-topup"]')).toContainText("Credit top-up from Platform");
      await page.screenshot({ path: test.info().outputPath(`${theme}-activity-settled-unknown.png`), animations: "disabled", caret: "hide" });

      await page.locator('button[data-txid="platform:moveToCore:pm-old:unknown"]').click();
      await expect(page.getByTestId("source-detail-title")).toHaveText(settled);
      await expect(page.getByTestId("source-detail-status")).toHaveText(text);
      await expect(page.getByTestId("source-detail-status")).not.toHaveClass(/is-problem/);
      await expect(page.getByTestId("source-detail-amount")).toHaveCount(0);
      await page.screenshot({ path: test.info().outputPath(`${theme}-activity-settled-unknown-older.png`), animations: "disabled", caret: "hide" });
      await page.keyboard.press("Escape");

      await page.locator('button[data-txid="platform:moveToCore:pm-move:unknown"]').click();
      await expect(page.getByTestId("source-detail-status")).toHaveText(text);
      await expect(page.getByTestId("source-detail-amount")).toContainText("0.3");
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test("a failed Shielded payment reads the same word in its details as on its row", async ({ page }) => {
    await activity(page);
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const s = f.world.pending.shielded!;
      s.activity = s.activity.map((a) => (a.id === "sh-2" ? { ...a, status: "failed" as const } : a));
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } } as never);
    });
    const row = page.locator('button[data-txid="shielded:sent:sh-2"]');
    await expect(row).toContainText("Not accepted");
    await row.click();
    await expect(page.getByTestId("source-detail-status")).toHaveText("Not accepted");
    await expect(page.getByTestId("source-detail-status")).toHaveClass(/is-problem/);
  });

  for (const theme of THEMES) {
    test(`a refused Shielded payment whose funds stay held may still go through, and offers no retry (${theme})`, async ({ page }) => {
      const errors = await activity(page, "funded", theme);
      const refuse = (status: "pending" | "failed", refusal: "held" | "released") =>
        page.evaluate(
          ([status, refusal]) => {
            const f = window.__dwFixture!;
            const s = f.world.pending.shielded!;
            s.activity = s.activity.map((a) => (a.id === "sh-2" ? { ...a, blockHeight: null, status, refusal } : a));
            f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } } as never);
          },
          [status, refusal] as const,
        );
      await refuse("pending", "held");
      const held = page.locator('button[data-txid="shielded:sent:sh-2:mayland"]');
      await expect(held).toContainText("May still go through");
      await expect(held).not.toContainText("Not accepted");
      await expect(held).toContainText("-0.1");
      await held.click();
      const sheet = page.getByTestId("source-detail");
      await expect(page.getByTestId("source-detail-status")).toHaveText("May still go through");
      await expect(page.getByTestId("source-detail-status")).not.toHaveClass(/is-problem/);
      await expect(page.getByTestId("source-detail-amount")).toContainText("-0.1");
      await expect(page.getByTestId("source-detail-refusal")).toContainText("may still accept it later");
      await expect(page.getByTestId("source-detail-refusal")).toContainText("Don't send this payment again");
      await expect(page.getByTestId("source-detail-refusal")).toContainText("(0.10000000 tDASH)");
      await expect(sheet.getByRole("button", { name: /retry|try again|send again|pay again/i })).toHaveCount(0);
      expect(await seriousAxeViolations(page)).toEqual([]);

      // Released while its details are open: the same record, now not accepted.
      await refuse("failed", "released");
      await expect(page.getByTestId("source-detail-status")).toHaveText("Not accepted");
      await expect(page.getByTestId("source-detail-status")).toHaveClass(/is-problem/);
      await expect(page.getByTestId("source-detail-refusal")).toHaveText("Dash Platform didn't accept this payment, and it can no longer go through, so DashPay Desktop has stopped holding the Shielded funds it used. Any of those funds that another payment hasn't spent are spendable again.");
      await page.keyboard.press("Escape");
      await expect(page.locator('button[data-txid="shielded:sent:sh-2"]')).toContainText("Not accepted");
      expect(errors).toEqual([]);
    });
  }

  test("says so when the record leaves the list while its details are open", async ({ page }) => {
    await activity(page);
    await page.locator('button[data-txid="shielded:received:sh-3"]').click();
    await expect(page.getByTestId("source-detail-memo")).toContainText("Lunch");
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.activity = f.world.pending.shielded!.activity.filter((a) => a.id !== "sh-3");
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } } as never);
    });
    await expect(page.getByTestId("source-detail-gone")).toHaveText("This activity is no longer listed.");
  });
});
