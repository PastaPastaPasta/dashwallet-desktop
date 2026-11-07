import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";

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

  test("paddr-recovery-restored: Home offers the Add from Dash wallet lock, and Finish adds it to the Platform address", async ({ page }) => {
    const errors = await open(page, "paddr-recovery-restored");
    const card = page.getByTestId("unfinished-card");
    await expect(card).toContainText("0.50 tDASH was funded on Core to add to your Platform address, and this wallet hasn't seen it added on Platform.");
    await expect(card).not.toContainText("Shielded");
    expect(await seriousAxeViolations(page)).toEqual([]);
    const platformBefore = await page.getByTestId("platform-card-amount").textContent();
    await page.getByTestId("unfinished-finish").click();
    await authorize(page);
    await expect(card).toHaveCount(0);
    await expect(page.getByTestId("platform-card-amount")).not.toHaveText(platformBefore!);
    await expect(page.getByTestId("platform-card-amount")).toContainText("0.89944");
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
    await expect(page.getByTestId("platform-card-amount")).toContainText("0.64944");
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
      await expect(page.getByTestId("shielded-card")).toBeVisible();
      await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
    });
  }
});
