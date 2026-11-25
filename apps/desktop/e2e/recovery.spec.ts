import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";
import { hold, release } from "./transfer";

// Unfinished transfers on Home (src/screens/UnfinishedTransfersCard.tsx) on the fixture backend
// (src/fixture/recovery.ts): the card for a restored Core → Shielded or "Add from Dash wallet" lock,
// and its Details sheet.

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero").waitFor();
  return errors;
}

test.describe("Unfinished transfers", () => {
  test("recovery-one: the card offers Finish, and the card goes once it is applied", async ({ page }) => {
    const errors = await open(page, "recovery-one");
    const card = page.getByTestId("unfinished-card");
    await expect(card).toContainText("0.50 tDASH was funded on Core for Shielded, and this wallet hasn't seen it added on Platform.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(card).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("recovery-recoverable: Home offers Recover for this device's refused transfer to Shielded, confirms the fresh amount, and recovers it", async ({ page }) => {
    const errors = await open(page, "recovery-recoverable");
    const card = page.getByTestId("unfinished-card");
    await expect(page.getByTestId("unfinished-text")).toHaveText(
      "0.30 tDASH is left on a transfer to Shielded that Platform refused. You can recover it to your Platform balance, minus Platform's fee.",
    );
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-recover").click();
    const confirm = page.getByTestId("recover-confirm");
    await expect(confirm.getByTestId("recover-confirm-text")).toHaveText("Move the 0.30 tDASH left on this transfer to your Platform balance? Platform's fee comes out of this amount.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await confirm.getByTestId("recover-confirm-cancel").click();
    await expect(confirm).toHaveCount(0);
    await page.getByTestId("unfinished-recover").click();
    await page.getByTestId("recover-confirm-ok").click();
    await authorize(page);
    await expect(card).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Recovered from an unfinished transfer").first()).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("recovery-recoverable-small: the card counts the recoverable row only; Details shows the below-fee row too small", async ({ page }) => {
    await open(page, "recovery-recoverable-small");
    await expect(page.getByTestId("unfinished-text")).toContainText("0.30 tDASH is left on a transfer to Shielded");
    await page.getByTestId("unfinished-details").click();
    const rows = page.getByTestId("unfinished-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).getByTestId("unfinished-row-state")).toHaveText("Can be recovered");
    await expect(rows.nth(1).getByTestId("unfinished-row-state")).toHaveText("Too small to finish: worth less than the Platform fee");
    await expect(rows.nth(1).getByTestId("unfinished-row-too-small")).toContainText("left on this transfer is less than Platform's fee for moving it, so it can't be recovered.");
    await expect(rows.nth(1).getByTestId("unfinished-row-recover")).toHaveCount(0);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });

  test("recovery-recoverable-mainnet: what is left is listed with the test-networks line, and there is no Recover anywhere", async ({ page }) => {
    await open(page, "recovery-recoverable-mainnet");
    await expect(page.getByTestId("unfinished-text")).toContainText("This version can recover it on test networks only.");
    await expect(page.getByTestId("unfinished-recover")).toHaveCount(0);
    await page.getByTestId("unfinished-details").click();
    const rows = page.getByTestId("unfinished-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.getByTestId("unfinished-row-state")).toHaveText(["Test networks only", "Test networks only"]);
    await expect(page.getByTestId("unfinished-intro-recover")).toHaveText("Platform refused these transfers to Shielded after charging a penalty. What's left is still yours.");
    await expect(page.getByTestId("unfinished-intro")).toHaveCount(0);
    await expect(page.getByTestId("unfinished-footer")).toHaveCount(0);
    await expect(page.getByTestId("unfinished-footer-recover")).toHaveCount(0);
    await expect(rows.getByTestId("unfinished-row-amount")).toHaveText(["0.30 DASH left of 0.50 DASH", "0.20 DASH left of 0.40 DASH"]);
    await expect(page.getByTestId("unfinished-row-recover")).toHaveCount(0);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });

  test("recovery-recoverable-mixed: the sheet shows the finish lines then the recover lines, and the refused row what is left", async ({ page }) => {
    await open(page, "recovery-recoverable-mixed");
    await page.getByTestId("unfinished-details").click();
    const sheet = page.getByTestId("unfinished-sheet");
    await expect(sheet.getByTestId("unfinished-intro")).toHaveText(
      "These transfers were funded on Core before the restore, and this wallet hasn't seen them added on Platform. Finishing adds each at most once.",
    );
    await expect(sheet.getByTestId("unfinished-intro-recover")).toHaveText(
      "Platform refused these transfers to Shielded after charging a penalty. Recover moves what's left of each to your Platform balance, minus Platform's fee.",
    );
    await expect(sheet.locator("[data-testid^=unfinished-intro]")).toHaveCount(2);
    expect(await sheet.locator("[data-testid^=unfinished-intro]").evaluateAll((ps) => ps.map((p) => p.getAttribute("data-testid")))).toEqual(["unfinished-intro", "unfinished-intro-recover"]);
    await expect(sheet.getByTestId("unfinished-footer")).toHaveText("Nothing more is paid: finishing only adds money that's already locked.");
    await expect(sheet.getByTestId("unfinished-footer-recover")).toHaveText("Recovering pays only Platform's fee, taken from what's left.");
    const amounts = sheet.getByTestId("unfinished-row-amount");
    await expect(amounts).toHaveText(["0.25 tDASH", "0.30 tDASH left of 0.50 tDASH"]);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });

  test("recovery-too-small-only: no card when every refused transfer is below the fee", async ({ page }) => {
    await open(page, "recovery-too-small-only");
    await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  });

  test("recovery-recover-fails: a recover that may have gone through says so and keeps the lock, and the next one recovers it", async ({ page }) => {
    await open(page, "recovery-recover-fails");
    await page.getByTestId("unfinished-recover").click();
    await page.getByTestId("recover-confirm-ok").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-note")).toHaveText("Dash Wallet can't tell yet whether the recovery went through. Trying again is safe: the same amount can't be recovered twice.");
    await page.getByTestId("unfinished-recover").click();
    await page.getByTestId("recover-confirm-ok").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-text")).toHaveCount(0);
  });

  test("recovery-one: a Finish still pending after a second says it is preparing the transfer", async ({ page }) => {
    const errors = await open(page, "recovery-one");
    const card = page.getByTestId("unfinished-card");
    await hold(page, "finish_transfer");
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(card).toHaveAttribute("data-kind", "running");
    await expect(card.getByTestId("transfer-preparing")).toHaveText("Preparing the transfer…");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await release(page);
    await expect(card).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("recovery-restored-mixed: Details lists the three restored locks", async ({ page }) => {
    await open(page, "recovery-restored-mixed");
    await expect(page.getByTestId("unfinished-text")).toContainText("2 transfers to Shielded (");
    await page.getByTestId("unfinished-details").click();
    await expect(page.getByTestId("unfinished-row")).toHaveCount(3);
    await expect(page.getByTestId("unfinished-footer")).toHaveText("Nothing more is paid: finishing only adds money that's already locked.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-row-finish").first().click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-row-result").first()).toHaveText("Added to Shielded");
  });

  test("recovery-finish-fails: the failure shows with Retry, and the retry applies", async ({ page }) => {
    await open(page, "recovery-finish-fails");
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-text")).toContainText("Dash Platform did not answer");
    await page.getByTestId("unfinished-retry").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  });

  test("recovery-checking-two: the card says the plural Checking line", async ({ page }) => {
    await open(page, "recovery-checking-two");
    await expect(page.getByTestId("unfinished-text")).toHaveText(
      "2 transfers may already be finished. Choose Check again to confirm them. If an amount wasn't added yet, Check again tries to add it. If any still show Checking, try again later.",
    );
  });

  test("recovery-checking: Check again reaches the backend, and the second try applies the lock", async ({ page }) => {
    await open(page, "recovery-checking");
    await expect(page.getByTestId("unfinished-text")).toContainText("1 transfer may already be finished");
    await page.getByTestId("unfinished-checkAgain").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-text")).toContainText("1 transfer may already be finished");
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.pending.recovery.checks)).toBe(1);
    await page.getByTestId("unfinished-checkAgain").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  });

  for (const scenario of ["recovery-restored-finished", "paddr-recovery-restored-finished"]) {
    test(`${scenario}: a restored lock Platform already took reads checking, and Check again says it was already added`, async ({ page }) => {
      const errors = await open(page, scenario);
      const text = page.getByTestId("unfinished-text");
      await expect(text).toHaveText("1 transfer may already be finished. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.");
      await expect(page.getByTestId("unfinished-card")).not.toContainText("hasn't seen");
      await page.getByTestId("unfinished-checkAgain").click();
      await authorize(page);
      await expect(text).toHaveText("The network says this was already added. There's nothing left to finish.");
      expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.getByTestId("unfinished-dismiss").click();
      await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

  test("recovery-restored-mixed: Check again from Details on a lock Platform already took keeps the row with the network's answer", async ({ page }) => {
    await open(page, "recovery-restored-mixed");
    await page.evaluate(() => {
      const r = window.__dwFixture!.world.pending.recovery;
      r.consumed = r.locks.filter((l) => l.state === "checking").map((l) => l.txid);
    });
    await page.getByTestId("unfinished-details").click();
    await expect(page.getByTestId("unfinished-row")).toHaveCount(3);
    await page.getByTestId("unfinished-row-checkAgain").click();
    await authorize(page);
    const row = page.locator('[data-testid="unfinished-row"][data-state="reportedUsed"]');
    await expect(row.getByTestId("unfinished-row-result")).toHaveText("The network says this was already added. There's nothing left to finish.");
    await expect(row.getByTestId("unfinished-row-checkAgain")).toHaveCount(0);
  });

  test("recovery-restored-unapplied: a restored checking lock Platform never took is added by Check again", async ({ page }) => {
    const errors = await open(page, "recovery-restored-unapplied");
    await expect(page.getByTestId("unfinished-text")).toHaveText("1 transfer may already be finished. Choose Check again to confirm it. If its amount wasn't added yet, Check again tries to add it. If it still shows Checking, try again later.");
    await page.getByTestId("unfinished-checkAgain").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
    expect(errors).toEqual([]);
  });

  for (const [scenario, outcome] of [
    ["recovery-restored-unapplied", "1 resumed, 1 completed, 0 still pending."],
    ["recovery-restored-finished", "1 resumed, 0 completed, 0 still pending, 1 already used."],
  ]) {
    test(`${scenario}: Tools ▸ Repair ▸ Finish transfers tries a restored checking lock`, async ({ page }) => {
      await open(page, scenario);
      await page.getByTestId("unfinished-text").waitFor();
      await page.evaluate(() => {
        (document.activeElement as HTMLElement | null)?.blur();
        window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" });
      });
      await expect(page.getByTestId("tools-tabs-repair")).toHaveAttribute("aria-selected", "true");
      await page.getByTestId("repair-finish-transfers").click();
      await authorize(page);
      await expect(page.getByTestId("repair-outcome")).toHaveText(outcome);
      expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
    });
  }

  test("paddr-recovery-restored: Home offers the Add from Dash wallet lock, and Finish adds it to the Platform address", async ({ page }) => {
    const errors = await open(page, "paddr-recovery-restored");
    const card = page.getByTestId("unfinished-card");
    await expect(card).toContainText("0.50 tDASH was funded on Core to add to your Platform address, and this wallet hasn't seen it added on Platform.");
    await expect(card).not.toContainText("Shielded");
    expect(await seriousAxeViolations(page)).toEqual([]);
    const platformBefore = await page.getByTestId("breakdown-platform").textContent();
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(card).toHaveCount(0);
    await expect(page.getByTestId("breakdown-platform")).not.toHaveText(platformBefore!);
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.89944");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Moved from Dash wallet").first()).toBeVisible();
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.filter((a) => a.kind === "fromCore" && a.state === "done").length)).toBe(1);
    expect(errors).toEqual([]);
  });

  test("paddr-recovery-mixed: the card names no one destination, Details lists both kinds, and Finish all applies both", async ({ page }) => {
    const errors = await open(page, "paddr-recovery-mixed");
    await expect(page.getByTestId("unfinished-text")).toContainText("2 transfers (");
    await expect(page.getByTestId("unfinished-text")).toContainText("were funded on Core, and this wallet hasn't seen them added on Platform.");
    await page.getByTestId("unfinished-details").click();
    await expect(page.getByTestId("unfinished-row")).toHaveCount(2);
    await expect(page.getByTestId("unfinished-row").nth(0)).toContainText("Shielded");
    await expect(page.getByTestId("unfinished-row").nth(1)).toContainText("Add from Dash wallet");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("unfinished-close").click();
    await page.getByTestId("unfinished-finishAll").click();
    await authorize(page);
    const text = page.getByTestId("unfinished-text");
    await expect(text).toContainText("2 resumed, 2 completed, 0 still pending.");
    await expect(text).toContainText("Shielded: 1 completed.");
    await expect(text).toContainText("Add from Dash wallet: 1 completed.");
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.64944");
    expect(errors).toEqual([]);
  });

  test("paddr-recovery-mixed: Details finishes the Add from Dash wallet row on its own", async ({ page }) => {
    await open(page, "paddr-recovery-mixed");
    await page.getByTestId("unfinished-details").click();
    await page.getByTestId("unfinished-row").nth(1).getByTestId("unfinished-row-finish").click();
    await authorize(page);
    await expect(page.getByTestId("unfinished-row-result").first()).toHaveText("Added to your Platform address");
    await expect(page.getByTestId("unfinished-row")).toHaveCount(2);
  });

  test("Repair ▸ Finish transfers applies a restored Add from Dash wallet lock and names it", async ({ page }) => {
    const errors = await open(page, "paddr-recovery-restored");
    await page.evaluate(() => {
      (document.activeElement as HTMLElement | null)?.blur();
      window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" });
    });
    await expect(page.getByTestId("tools-tabs-repair")).toHaveAttribute("aria-selected", "true");
    await page.getByTestId("repair-finish-transfers").click();
    await authorize(page);
    await expect(page.getByTestId("repair-outcome")).toHaveText("1 resumed, 1 completed, 0 still pending. Add from Dash wallet: 1 completed.");
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
    expect(errors).toEqual([]);
  });

  test("paddr-recovery-shielded-unread: the Add from Dash wallet lock is offered and finished while Shielded can't be checked", async ({ page }) => {
    const errors = await open(page, "paddr-recovery-shielded-unread");
    const card = page.getByTestId("unfinished-card");
    await expect(card).toContainText("0.50 tDASH was funded on Core to add to your Platform address");
    await page.evaluate(() => {
      (document.activeElement as HTMLElement | null)?.blur();
      window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" });
    });
    await expect(page.getByTestId("repair-unfinished-unread")).toHaveText("Shielded transfers couldn't be checked; try again.");
    await page.getByTestId("repair-finish-transfers").click();
    await authorize(page);
    await expect(page.getByTestId("repair-outcome")).toHaveText(
      "1 resumed, 1 completed, 0 still pending. Add from Dash wallet: 1 completed. Shielded transfers couldn't be checked; try again.",
    );
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.recovery.locks.length)).toBe(0);
    expect(errors).toEqual([]);
  });

  for (const scenario of ["funded", "recovery-confirming", "recovery-not-active"]) {
    test(`${scenario}: Home has no card`, async ({ page }) => {
      await open(page, scenario);
      await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
      await expect(page.getByTestId("shielded-card")).toHaveCount(0);
      await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
    });
  }
});
