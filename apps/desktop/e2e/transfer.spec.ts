import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, openTransfer, trackPageErrors } from "./helpers";
import { callsOf, FEE_ON_NEXT_STEP, hold, lastGrant, release, TRANSFER_STATES } from "./transfer";

// Transfer (POLISH 25): one sheet over the four routes' own flows. Each state of transfer.ts with
// no serious axe violation and no page error; each route's grant as its own sheet asked for it.

async function open(page: Page, scenario: string, extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light", extra));
  await page.getByTestId("hero").waitFor();
  return errors;
}

test.describe("Transfer states", () => {
  for (const s of TRANSFER_STATES) {
    test(s.id, async ({ page }) => {
      const errors = await open(page, s.scenario, s.extra);
      await s.go(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
});

/** Through the route's confirmation to its passphrase sheet; the grant asked for, once answered. */
async function grantOf(page: Page, go: string) {
  await page.getByTestId("transfer-continue").click();
  await page.getByTestId(go).click();
  await expect(page.getByTestId("grant-sheet")).toBeVisible();
  await expect(page.getByTestId("grant-sheet")).toContainText("Enter your passphrase to authorize this payment.");
  await authorize(page);
  return lastGrant(page);
}

test.describe("Transfer grants", () => {
  test("Dash wallet → Shielded: a platformOp grant of the lock and its fee bound", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").fill("0.001");
    expect(await grantOf(page, "shielded-go")).toEqual({ kind: "platformOp", maxDuffs: 101_000, maxCredits: "0" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
  });

  test("Dash wallet → Platform: a platformOp grant of the lock and its fee bound", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToPlatform");
    await page.getByTestId("transfer-amount").fill("0.5");
    expect(await grantOf(page, "platform-fund-go")).toEqual({ kind: "platformOp", maxDuffs: 50_001_000, maxCredits: "0" });
    await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
  });

  test("Shielded → Dash wallet: a platformOp grant of the credits and the fee", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "shieldedToCore");
    await page.getByTestId("transfer-amount").fill("0.5");
    expect(await grantOf(page, "shielded-go")).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "50020000000" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
  });

  test("Platform → Dash wallet: a platformOp grant of the whole balance", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "platformToCore");
    expect(await grantOf(page, "platform-move-go")).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "40000000000" });
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
  });

  test("a card's Finish opens Transfer at its route, with the one-duff grant", async ({ page }) => {
    await open(page, "transfer-shield-pending");
    await page.getByTestId("shielded-finish").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "coreToShielded");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-view", "steps");
    await authorize(page);
    expect(await lastGrant(page)).toEqual({ kind: "platformOp", maxDuffs: 1, maxCredits: "0" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
  });
});

test.describe("Transfer quotes", () => {
  test("typing reads one quote, for the amount it settles on", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").pressSequentially("0.0015", { delay: 40 });
    await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about /);
    expect(await callsOf(page, "shield_quote")).toBe(1);
  });

  test("Dash wallet → Platform: typing makes no platform_fund_quote call, Continue exactly one", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToPlatform");
    await page.getByTestId("transfer-amount").pressSequentially("0.55", { delay: 40 });
    await expect(page.getByTestId("transfer-fee")).toHaveText(FEE_ON_NEXT_STEP);
    // Well past the other routes' debounce: still nothing read.
    await page.waitForTimeout(1_500);
    expect(await callsOf(page, "platform_fund_quote")).toBe(0);
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("platform-fund-confirm")).toContainText("You receive about");
    expect(await callsOf(page, "platform_fund_quote")).toBe(1);
  });

  test("Max never skips the confirmation", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "shieldedToCore");
    await page.getByTestId("transfer-max").click();
    await expect(page.getByTestId("transfer-amount")).toHaveValue(/^1\.24980*$/);
    const grants = await page.evaluate(() => window.__dwFixture!.world.grants.length);
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-quote")).toBeVisible();
    await expect(page.getByTestId("shielded-go")).toBeEnabled();
    expect(await page.evaluate(() => window.__dwFixture!.world.grants.length)).toBe(grants);
    expect(await callsOf(page, "unshield_to_core")).toBe(0);
  });

  test("an unseen result's Show on a card opens Transfer at that route's result", async ({ page }) => {
    await open(page, "funded");
    await hold(page, "platform_move_to_core");
    await openTransfer(page, "platformToCore");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    // Closed while it runs: its result waits on the card.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await release(page);
    await expect(page.getByTestId("platform-move-unseen")).toBeVisible();
    await page.getByTestId("platform-move-show").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "platformToCore");
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
  });
});
