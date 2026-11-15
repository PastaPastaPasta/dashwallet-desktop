import { expect, test } from "@playwright/test";
import { authorize, fixtureUrl, openTransfer, platformMoveWhy, THEMES } from "./helpers";

/** The Home pool sections and their rows that only repeated what the sheets say. */
const POOL_CARDS = ["shielded-card", "platform-card", "platform-card-dust", "platform-card-waiting", "platform-card-send-held", "shielded-card-pending", "shielded-card-why"];

for (const theme of THEMES) {
  test(`Home has no pool cards in the normal state (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    for (const id of ["shielded-card", "platform-card", "shielded-card-text", "platform-card-amount", "shielded-card-transfer", "platform-card-transfer"]) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shortcut-transfer").click();
    await expect(page.getByTestId("transfer-sheet")).toBeVisible();
  });

  test(`Home has no pool card with only Platform dust, and Transfer says why (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("platform-dust-small", theme));
    // Nothing movable on Platform: Home has no Platform row.
    await expect(page.getByTestId("hero-amount")).toBeVisible();
    await expect(page.getByTestId("breakdown-platform")).toHaveCount(0);
    for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
    await expect(page.getByTestId("page-home")).not.toContainText("cannot be moved");
    expect(await platformMoveWhy(page)).toBe("0.004005 tDASH on your Platform addresses cannot be moved to your Dash wallet right now.");
  });

  test(`Unfinished transfers keeps both Finish paths, with no pool card (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("paddr-fund-proof-waiting", theme));
    const card = page.getByTestId("unfinished-card");
    await expect(card.getByTestId("platform-fund-unfinished")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }];
      f.emit({ kind: "eventsOverflowed" });
    });
    await expect(card.getByTestId("shielded-unfinished")).toBeVisible();
    await expect(card.getByRole("heading")).toHaveText("Unfinished transfers");
    for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-unfinished")).toHaveCount(0);
    await expect(card.getByTestId("platform-fund-unfinished")).toBeVisible();
    await page.getByTestId("platform-fund-finish").click();
    await authorize(page);
    await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
    await page.getByTestId("platform-fund-done").click();
    await expect(page.getByTestId("platform-fund-unfinished")).toHaveCount(0);
    await expect(card).toHaveCount(0);
  });

  for (const pool of ["shielded", "platform"] as const) {
    test(`Home retains an unseen ${pool} result and Show (${theme})`, async ({ page }) => {
      const command = pool === "shielded" ? "shield_from_core" : "platform_fund";
      const prefix = pool === "shielded" ? "shielded" : "platform-fund";
      await page.goto(fixtureUrl("funded", theme, `&hold=${command}`));
      await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
      await openTransfer(page, pool === "shielded" ? "coreToShielded" : "coreToPlatform");
      await page.getByTestId("transfer-amount").fill("0.001");
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId(`${prefix}-go`).click();
      await authorize(page);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      await page.evaluate((command) => window.__dwFixture!.release(command as "shield_from_core" | "platform_fund"), command);
      await expect(page.getByTestId("unfinished-card").getByTestId(`${prefix}-unseen`)).toBeVisible();
      for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
      await page.getByTestId(`${prefix}-show`).click();
      await expect(page.getByTestId(`${prefix}-outcome-title`)).toHaveText(pool === "shielded" ? "Added to Shielded" : "Added to Platform");
    });
  }
}

test("An open Shielded passphrase prompt survives the Home section appearing and going", async ({ page }) => {
  await page.goto(fixtureUrl("funded", THEMES[0]));
  await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
  await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  await openTransfer(page, "coreToShielded");
  await page.getByTestId("transfer-amount").fill("0.001");
  await page.getByTestId("transfer-continue").click();
  await page.getByTestId("shielded-go").click();
  const input = page.getByTestId("grant-sheet").locator("input[type=password]");
  await input.fill("de");
  await input.evaluate((el) => el.setAttribute("data-kept", "1"));
  const setShields = (n: number) =>
    page.evaluate((n) => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = n ? [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }] : [];
      f.emit({ kind: "eventsOverflowed" });
    }, n);
  await setShields(1);
  await expect(page.getByTestId("unfinished-card").getByTestId("shielded-unfinished")).toHaveCount(1);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await setShields(0);
  await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await input.pressSequentially("mo");
  await input.press("Enter");
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
});

for (const theme of THEMES) {
  test(`Home asks to join DashPay once, and the hero balance reads like the mobile wallets (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-no-identity", theme));
    const home = page.getByTestId("page-home");
    await expect(page.getByTestId("dashpay-card")).toHaveAttribute("data-state", "join");
    // One button on Home, the card's; the sidebar entry stays as navigation; the toolbar has none.
    await expect(home.getByRole("button", { name: "Join DashPay" })).toHaveCount(1);
    await expect(home.getByText("Join DashPay", { exact: true })).toHaveCount(1);
    await expect(page.getByTestId("toolbar-chip")).toHaveCount(0);
    await expect(page.getByTestId("sidebar-joinDashPay")).toHaveText("Join DashPay");
    await home.getByRole("button", { name: "Join DashPay" }).click();
    await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
    await page.getByTestId("sidebar-home").click();

    // The exact amount, with the decimals past the second drawn smaller.
    const number = page.getByTestId("hero-amount").locator(".amount-number");
    await expect(number).toHaveText("32.04698287");
    const minor = number.locator(".amount-minor");
    await expect(minor).toHaveText("698287");
    const size = (l: typeof number) => l.evaluate((e) => parseFloat(getComputedStyle(e).fontSize));
    expect(await size(minor)).toBeLessThan(await size(number));
    await expect(page.getByTestId("hero-amount").locator(".amount")).toHaveAttribute("title", "32.04698287 tDASH");
  });
}

test("Home's hero shows a round balance without a smaller tail, and no breakdown that repeats it", async ({ page }) => {
  await page.goto(fixtureUrl("pools-absent", "light"));
  await expect(page.getByTestId("hero-amount")).toContainText("30.39698287");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const w = f.world.wallets[0];
    w.summary = { ...w.summary, balances: { ...w.summary.balances!, confirmed: 125_000_000, unconfirmed: 0, immature: 0, locked: 0, total: 125_000_000 } };
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.summary.walletId, balances: w.summary.balances });
  });
  await expect(page.getByTestId("hero-amount").locator(".amount-number")).toHaveText("1.25");
  await expect(page.getByTestId("hero-amount").locator(".amount-minor")).toHaveCount(0);
  await expect(page.getByTestId("hero-breakdown")).toHaveCount(0);
});
