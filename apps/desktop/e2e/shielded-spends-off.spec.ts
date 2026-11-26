import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, openTransfer, THEMES, trackPageErrors, type Theme } from "./helpers";
import { callsOf } from "./transfer";

// Where this build cannot spend from Shielded (`ShieldedStatus.spendsAvailable` false): Send from
// Shielded, Pay's Shielded route and the moves out of Shielded wait with one reason line; every way
// into Shielded (Transfer from the Dash wallet or Platform, CoinJoin's move) with another, and
// Receive shows no Shielded address, with its own line. Nothing reaches the engine. The balance
// carries on. With the flag on (main today), every one of them is as it was.

const REASON = "This version of Dash Wallet can't send from your Shielded balance yet. Your Shielded balance stays in your wallet and keeps syncing, and sending from it comes in a later update.";
const SHIELD_IN = "This version of Dash Wallet doesn't move funds into Shielded, because it can't send from Shielded yet. Both come in a later update.";
const RECEIVE = "This version of Dash Wallet doesn't show a Shielded address, because it can't send from Shielded yet. Payments to a Shielded address you gave out earlier still arrive, and they show in your Shielded balance.";
const SHIELDED_ADDRESS = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";
const SPENDS = ["shielded_send", "shielded_send_quote", "unshield_to_core", "unshield_quote", "unshield_to_platform", "unshield_to_platform_quote", "shield_from_core", "shield_quote", "shield_mixed_coins", "shield_mixed_quote", "platform_shield_quote"];

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
    test(`every spend from Shielded and every way into it waits with its reason (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-spends-off", theme);
      await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");

      await page.getByTestId("sidebar-pay").click();
      expect(await reasonOf(page, "shielded-send")).toBe(REASON);
      await shot(page, `${theme}-pay-send-off`);

      await page.getByTestId("pay-tabs-address").click();
      await page.getByTestId("pay-amount").fill("0.2");
      await page.getByTestId("pay-address").fill(SHIELDED_ADDRESS);
      expect(await reasonOf(page, "pay-shielded-route")).toBe(REASON);
      await expect(page.getByTestId("pay-address-form").locator(".form-field").first().locator(".field-hint")).toHaveCount(0);
      await shot(page, `${theme}-pay-route-off`);

      await page.getByTestId("sidebar-home").click();
      for (const route of ["shieldedToCore", "shieldedToPlatform", "coreToShielded"] as const) {
        await openTransfer(page, route);
        await expect(page.getByTestId("transfer-unavailable")).toHaveText(route === "coreToShielded" ? SHIELD_IN : REASON);
        await expect(page.getByTestId("transfer-continue")).toBeDisabled();
        await shot(page, `${theme}-transfer-${route}-off`);
        expect(await seriousAxeViolations(page)).toEqual([]);
        await page.getByTestId("transfer-cancel").click();
      }
      await openTransfer(page, "platformToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-shielded").click();
      await expect(page.getByTestId("transfer-unavailable")).toHaveText(SHIELD_IN);
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      await shot(page, `${theme}-transfer-platformToShielded-off`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-body")).toHaveCount(0);

      await page.getByTestId("sidebar-receive").click();
      await page.getByTestId("shielded-receive-open").click();
      await expect(page.getByTestId("shielded-receive-spends-off")).toHaveText(RECEIVE);
      await expect(page.getByTestId("shielded-receive-address")).toHaveCount(0);
      await expect(page.getByTestId("shielded-receive-qr")).toHaveCount(0);
      await shot(page, `${theme}-receive-off`);
      expect(await seriousAxeViolations(page)).toEqual([]);

      expect(await spendCalls(page)).toBe(0);
      expect(await callsOf(page, "shielded_receive_address")).toBe(0);
      expect(errors).toEqual([]);
    });
  }

  for (const theme of THEMES) {
    test(`Tools ▸ Repair names no move into Shielded again (${theme})`, async ({ page }) => {
      const errors = await open(page, "repair-shield-spends-off", theme);
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" }));
      const blocked = page.getByTestId("repair-stuck-shield/1/funding");
      await expect(blocked).toContainText("Repair can't send these coins back: No payment of this move is known, so nothing shows which coins to send back.");
      await expect(blocked).not.toContainText("New moves into Shielded");
      const priced = page.getByTestId("repair-stuck-shield/0/funding");
      await expect(priced).toContainText(/Repair sends the move's 3 mixed coins \(0\.300003 t?DASH\) back to your wallet's main balance, at a network fee of 0\.00000488 t?DASH\./);
      await expect(priced).not.toContainText("into Shielded again");
      await expect(page.getByText("Whether these moves' payment reached the network is unknown. Repair settles each with a payment back to your own wallet; you confirm its coins and its fee first.", { exact: true })).toBeVisible();
      await expect(page.getByText("no new move into Shielded can start", { exact: false })).toHaveCount(0);
      await blocked.scrollIntoViewIfNeeded();
      await shot(page, `${theme}-repair-blocked-off`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await page.getByTestId("repair-resolve-shield/0/funding").click();
      await page.getByTestId("repair-confirm-ok").click();
      await authorize(page);
      const outcome = page.getByTestId("repair-outcome");
      await expect(outcome).toHaveText(/^The coins were sent back to your wallet \(transaction [0-9a-f]{64}\)\.$/);
      await outcome.scrollIntoViewIfNeeded();
      await shot(page, `${theme}-repair-resolved-off`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test("Tools ▸ Repair promises no move into Shielded while the pool reads inactive", async ({ page }) => {
    const errors = await open(page, "repair-shield-spends-off", "light");
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.active = false;
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } } as never);
    });
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" }));
    await expect(page.getByText("Whether these moves' payment reached the network is unknown. Repair settles each with a payment back to your own wallet; you confirm its coins and its fee first.", { exact: true })).toBeVisible();
    const priced = page.getByTestId("repair-stuck-shield/0/funding");
    await expect(priced).toContainText(/Repair sends the move's 3 mixed coins/);
    await expect(priced).not.toContainText("into Shielded again");
    await expect(page.getByTestId("repair-stuck-shield/1/funding")).not.toContainText("New moves into Shielded");
    await page.getByTestId("repair-resolve-shield/0/funding").click();
    await page.getByTestId("repair-confirm-ok").click();
    await authorize(page);
    await expect(page.getByTestId("repair-outcome")).toHaveText(/^The coins were sent back to your wallet \(transaction [0-9a-f]{64}\)\.$/);
    expect(errors).toEqual([]);
  });

  test("moving mixed coins into Shielded waits with the ways-in reason", async ({ page }) => {
    const errors = trackPageErrors(page);
    for (const scenario of ["coinjoin-move-spends-off", "coinjoin-move"] as const) {
      await page.goto(fixtureUrl(scenario, "light"));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
      if (scenario === "coinjoin-move-spends-off") {
        expect(await reasonOf(page, "coinjoin-move-shield")).toBe(SHIELD_IN);
        await expect(page.getByTestId("coinjoin-move-open")).toBeEnabled();
        expect(await seriousAxeViolations(page)).toEqual([]);
        expect(await spendCalls(page)).toBe(0);
      } else {
        await page.getByTestId("coinjoin-move-shield").click();
        await expect(page.getByTestId("move-mixed-body")).toHaveAttribute("data-step", "confirm");
        await expect(page.getByTestId("move-mixed-confirm")).toBeEnabled();
      }
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

  test("an added Shielded recipient isn't pointed to Send from Shielded", async ({ page }) => {
    const errors = await open(page, "shielded-spends-off", "light");
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
    await page.getByTestId("pay-add-recipient").click();
    await page.getByTestId("pay-more-2-address").fill(SHIELDED_ADDRESS);
    await expect(page.getByTestId("pay-more-2").locator(".field-error").first()).toHaveText("This is a shielded address. This wallet cannot pay shielded addresses yet.");
    expect(errors).toEqual([]);
  });

  test("a shield already sent can still be finished", async ({ page }) => {
    const errors = await open(page, "shielded-spends-off-pending", "light");
    await expect(page.getByTestId("shielded-finish")).toBeEnabled();
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    expect(await callsOf(page, "resume_shield")).toBe(1);
    expect(await spendCalls(page)).toBe(0);
    expect(errors).toEqual([]);
  });

  test("Join DashPay pay-with step uses Pay for your username and omits the Shielded recommendation", async ({ page }) => {
    const errors = await open(page, "shielded-spends-off", "light");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
    await page.getByTestId("join-create").click();
    const step = page.getByTestId("registration-pay-with");
    await expect(step).toHaveAttribute("data-kind", "sources");
    await expect(step.getByRole("heading", { level: 2 })).toHaveText("Pay for your username");
    await expect(page.getByTestId("privacy-body")).toHaveCount(0);
    await expect(page.getByTestId("pay-with-core-note")).toHaveText("Funds will be traceable to your username");
    await expect(page.getByTestId("pay-with-shielded")).toHaveCount(0);
    await expect(page.getByTestId("privacy-shield-first")).toHaveCount(0);
    expect(await seriousAxeViolations(page)).toEqual([]);
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
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("shielded-receive-open").click();
    await expect(page.getByTestId("shielded-receive-address")).toBeVisible();
    await expect(page.getByTestId("shielded-receive-spends-off")).toHaveCount(0);
    await page.getByTestId("sidebar-home").click();
    await toShieldedConfirm(page);
    for (const line of [REASON, SHIELD_IN, RECEIVE]) await expect(page.getByText(line)).toHaveCount(0);
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-body")).toHaveAttribute("data-step", "done");
    expect(await callsOf(page, "shield_from_core")).toBe(1);
    expect(errors).toEqual([]);
  });
});
