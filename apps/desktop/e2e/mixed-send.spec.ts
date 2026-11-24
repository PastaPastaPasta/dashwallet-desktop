import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// U-06b: a payment from fully mixed coins says, on its confirm sheet, how many mixed coins it
// spends, warns from 10 of them, and names the left-over that goes to the fee. To an address, to a
// contact and Advanced's unsigned transaction; no other coin source says any of it.

test.describe.configure({ timeout: 90_000 });

const DENOM = 10_000_100;

async function open(page: Page, theme: Theme, tab: "address" | "contact") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", theme));
  await page.getByTestId("toolbar").waitFor();
  // One unmixed 0.5 coin and twelve fully mixed 0.100001 coins.
  await page.evaluate(`(() => {
    const f = window.__dwFixture;
    const wallet = f.world.wallets[0];
    const c = wallet.coins[0];
    const coin = (vout, amount, extra) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount, spendable: true, confirmations: 10, ...extra });
    wallet.coins = [coin(1, 50_000_000, { coinjoinDenominated: false, coinjoinRounds: null })];
    for (let i = 0; i < 12; i++) wallet.coins.push(coin(10 + i, ${DENOM}, { coinjoinDenominated: true, coinjoinRounds: 4 }));
    f.world.coinjoin.mixers[wallet.summary.walletId] = {
      state: "idle", stopReason: null, status: { kind: "idle" },
      progress: { overallPercent: 100, denominatedPercent: 100, partiallyMixedPercent: 100, mixedPercent: 100, averageRounds: 4 },
      sessions: [], queueSize: 0,
    };
    f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: wallet.summary.walletId });
  })()`);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId(`pay-tabs-${tab}`).click();
  await fontsReady(page);
  return errors;
}

async function payee(page: Page, tab: "address" | "contact") {
  if (tab === "address") await page.getByTestId("pay-address").fill(PAYEE);
  else await page.getByTestId("pay-frequent-chip").first().click();
}

async function mixedCoins(page: Page) {
  await page.getByTestId("pay-advanced").locator("summary").click();
  await page.getByTestId("coin-choice-mixed").check();
}

async function review(page: Page) {
  await page.getByTestId("pay-fee-line").waitFor();
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).click();
  await page.getByTestId("pay-confirm").waitFor();
}

const sheet = (page: Page) => page.getByTestId("pay-confirm");
const MANY =
  "Spending 10 or more mixed coins in one payment can link them together and weaken your privacy. Consider paying a smaller amount.";

/** 1.05 from 0.100001 coins: eleven of them, and what they hold beyond it and the fee is fee too. */
async function expectElevenCoins(page: Page) {
  const warning = page.getByTestId("pay-confirm-mixed-warning");
  await expect(warning).toHaveText(MANY);
  await expect(warning).toHaveAttribute("role", "alert");
  await expect(page.getByTestId("pay-confirm-mixed-coins")).toHaveText("This payment spends 11 mixed coins.");
  // 11 inputs, 1 output: 1672 bytes at 1000 duffs/kB. The rest of 1.10001100 is the left-over.
  const leftOver = ((11 * DENOM - 105_000_000 - 1672) / 1e8).toFixed(8);
  await expect(page.getByTestId("pay-confirm-mixed-change")).toHaveText(
    `Mixed payments have no change: the ${leftOver} tDASH left over after the payment goes to the network fee.`,
  );
  // The fee line already holds it.
  await expect(page.getByTestId("pay-confirm-fee")).toContainText(((11 * DENOM - 105_000_000) / 1e8).toFixed(8));
}

for (const theme of THEMES) {
  for (const tab of ["address", "contact"] as const) {
    test(`U-06b to ${tab === "address" ? "an address" : "a contact"} ${theme}: eleven mixed coins warn and name the left-over`, async ({ page }, info) => {
      const errors = await open(page, theme, tab);
      await payee(page, tab);
      await page.getByTestId("pay-amount").fill("1.05");
      await mixedCoins(page);
      await review(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-mixed-send-${tab}-eleven.png`) });
      await expectElevenCoins(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test(`U-06b unsigned ${theme}: Advanced's unsigned transaction from mixed coins says the same`, async ({ page }, info) => {
    const errors = await open(page, theme, "address");
    await payee(page, "address");
    await page.getByTestId("pay-amount").fill("1.05");
    await mixedCoins(page);
    await page.getByTestId("pay-advanced-psbt").locator("summary").click();
    await page.getByTestId("pay-create-psbt").and(page.locator(":enabled")).click();
    await sheet(page).waitFor();
    await expect(sheet(page).getByRole("heading")).toHaveText("Unsigned transaction");
    await page.screenshot({ path: artifactPath(info, `${theme}-mixed-send-unsigned-eleven.png`) });
    await expectElevenCoins(page);
    expect(errors).toEqual([]);
  });

  test(`U-06b one coin ${theme}: a mixed coin spent whole gives no warning and no left-over note`, async ({ page }, info) => {
    const errors = await open(page, theme, "address");
    await payee(page, "address");
    await page.getByTestId("pay-amount").fill("0.100001");
    await page.getByTestId("pay-subtract-fee").check();
    await mixedCoins(page);
    await review(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-mixed-send-address-one.png`) });
    await expect(page.getByTestId("pay-confirm-mixed-coins")).toHaveText("This payment spends 1 mixed coin.");
    await expect(page.getByTestId("pay-confirm-mixed-warning")).toHaveCount(0);
    await expect(page.getByTestId("pay-confirm-mixed-change")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("U-06b any funds: a normal payment says nothing about mixed coins", async ({ page }) => {
  const errors = await open(page, "light", "address");
  await payee(page, "address");
  await page.getByTestId("pay-amount").fill("0.2");
  await review(page);
  await expect(sheet(page)).toContainText("Any available funds");
  await expect(page.locator('[data-testid^="pay-confirm-mixed"]')).toHaveCount(0);
  expect(errors).toEqual([]);
});
