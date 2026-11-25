import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, openTransfer, THEMES, trackPageErrors, type Theme } from "./helpers";
import { callsOf } from "./transfer";

// Where this build cannot spend from Shielded (`ShieldedStatus.spendsAvailable` false): Send from
// Shielded, Pay's Shielded route and the moves out of Shielded wait with one reason line, nothing
// reaches the engine, and a move into Shielded says what it means. Shielding and the balance carry
// on. With the flag on (main today), every one of them is as it was.

const REASON = "This version of Dash Wallet can't send from your Shielded balance yet. Your Shielded balance stays in your wallet and keeps syncing, and sending from it comes in a later update.";
const SHIELD_IN = "You can't send from Shielded in this version yet, so anything you move there stays there until a later update.";
const SHIELDED_ADDRESS = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";
const SPENDS = ["shielded_send", "shielded_send_quote", "unshield_to_core", "unshield_quote", "unshield_to_platform", "unshield_to_platform_quote"];

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
  await page.getByTestId("hero-amount").waitFor();
  return errors;
}

async function reasonOf(page: Page, testId: string) {
  const button = page.getByTestId(testId);
  await expect(button).toBeDisabled();
  const id = await button.getAttribute("aria-describedby");
  expect(id, `${testId} is described`).toBeTruthy();
  return (await page.locator(`[id="${id}"]`).textContent())!.trim();
}

async function spendCalls(page: Page) {
  let n = 0;
  for (const c of SPENDS) n += await callsOf(page, c);
  return n;
}

async function toShieldedConfirm(page: Page) {
  await openTransfer(page, "coreToShielded");
  await page.getByTestId("transfer-amount").fill("0.1");
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  await page.getByTestId("transfer-continue").click();
  await expect(page.getByTestId("shielded-quote")).toBeVisible();
}

test.describe("Shielded spends off", () => {
  for (const theme of THEMES) {
    test(`every spend from Shielded waits with one reason, and shielding says what it means (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-spends-off", theme);
      await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");

      await page.getByTestId("sidebar-pay").click();
      expect(await reasonOf(page, "shielded-send")).toBe(REASON);
      await shot(page, `${theme}-pay-send-off`);

      await page.getByTestId("pay-tabs-address").click();
      await page.getByTestId("pay-amount").fill("0.2");
      await page.getByTestId("pay-address").fill(SHIELDED_ADDRESS);
      expect(await reasonOf(page, "pay-shielded-route")).toBe(REASON);
      await shot(page, `${theme}-pay-route-off`);

      await page.getByTestId("sidebar-home").click();
      for (const route of ["shieldedToCore", "shieldedToPlatform"] as const) {
        await openTransfer(page, route);
        await expect(page.getByTestId("transfer-unavailable")).toHaveText(REASON);
        await expect(page.getByTestId("transfer-continue")).toBeDisabled();
        await shot(page, `${theme}-transfer-${route}-off`);
        expect(await seriousAxeViolations(page)).toEqual([]);
        await page.getByTestId("transfer-cancel").click();
      }

      await toShieldedConfirm(page);
      await expect(page.getByTestId("shielded-spends-off-in")).toHaveText(SHIELD_IN);
      await expect(page.getByTestId("shielded-go")).toBeEnabled();
      await shot(page, `${theme}-transfer-coreToShielded-confirm`);
      expect(await seriousAxeViolations(page)).toEqual([]);

      expect(await spendCalls(page)).toBe(0);
      expect(errors).toEqual([]);
    });
  }

  test("moving mixed coins into Shielded says what it means", async ({ page }) => {
    const errors = trackPageErrors(page);
    for (const [scenario, lines] of [["coinjoin-move-spends-off", 1], ["coinjoin-move", 0]] as const) {
      await page.goto(fixtureUrl(scenario, "light"));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
      await page.getByTestId("coinjoin-move-shield").click();
      await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
      await expect(page.getByTestId("move-mixed-confirm")).toBeEnabled();
      if (lines) await expect(page.getByTestId("move-mixed-spends-off-in")).toHaveText(SHIELD_IN);
      else await expect(page.getByTestId("move-mixed-spends-off-in")).toHaveCount(0);
    }
    expect(errors).toEqual([]);
  });

  test("Pay a contact's Shielded row says this version can't pay from it", async ({ page }) => {
    const errors = trackPageErrors(page);
    for (const [scenario, reason] of [["dashpay-registered-spends-off", "Not available in this version"], ["dashpay-registered", "Not available on this network yet."]] as const) {
      await page.goto(fixtureUrl(scenario, "light"));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
      await page.getByTestId("pay-frequent-chip").first().click();
      await expect(page.getByTestId("pay-from-shielded-reason")).toHaveText(reason);
      await expect(page.getByTestId("pay-from").getByRole("radio", { name: /Shielded balance/ })).toBeDisabled();
    }
    expect(errors).toEqual([]);
  });

  test("a move into Shielded still runs", async ({ page }) => {
    const errors = await open(page, "shielded-spends-off", "light");
    await toShieldedConfirm(page);
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-body")).toHaveAttribute("data-step", "done");
    expect(await callsOf(page, "shield_from_core")).toBe(1);
    expect(errors).toEqual([]);
  });

  test("with spends on, as main's engine says, nothing waits for this reason", async ({ page }) => {
    const errors = await open(page, "funded", "light");
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByTestId("shielded-send")).toBeEnabled();
    await page.getByTestId("sidebar-home").click();
    for (const route of ["shieldedToCore", "shieldedToPlatform"] as const) {
      await openTransfer(page, route);
      await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
      await page.getByTestId("transfer-cancel").click();
    }
    await toShieldedConfirm(page);
    await expect(page.getByTestId("shielded-spends-off-in")).toHaveCount(0);
    await expect(page.getByText(REASON)).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});
