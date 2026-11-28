import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Lane F: the confirm sheet says a normal or selected payment is not private while CoinJoin
// mixes or the wallet holds fully mixed coins, and only when the payment spends a coin that is
// not fully mixed.

const PAYEE = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";

async function setUp(page: Page, theme: Theme, state: "idle" | "mixing", mixedCoins: number) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  await page.evaluate(
    ([state, mixedCoins]) => {
      const w = window.__dwFixture!.world;
      const wallet = w.wallets[0];
      const c = wallet.coins[0];
      const coin = (vout: number, amount: number, extra: object) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount, spendable: true, confirmations: 10, ...extra });
      wallet.coins = [
        coin(0, 50_000_000, { coinjoinDenominated: false, coinjoinRounds: null }),
        ...Array.from({ length: mixedCoins as number }, (_, i) => coin(i + 1, 10_000_100, { coinjoinDenominated: true, coinjoinRounds: 8 })),
      ];
      w.coinjoin.mixers[wallet.summary.walletId] = {
        state: state as "idle" | "mixing",
        stopReason: null,
        status: { kind: state === "mixing" ? "mixingInProgress" : "idle" },
        progress: mixedCoins
          ? { overallPercent: 50, denominatedPercent: 100, partiallyMixedPercent: 50, mixedPercent: 50, averageRounds: 4 }
          : { overallPercent: 0, denominatedPercent: 0, partiallyMixedPercent: 0, mixedPercent: 0, averageRounds: 0 },
        sessions: [],
        queueSize: 0,
      };
    },
    [state, mixedCoins] as const,
  );
  await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId }); })()`);
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-address").fill(PAYEE);
}

async function review(page: Page) {
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
}

const NOTE =
  "This payment uses coins that aren't mixed, so it can be traced back to where they came from. To pay privately, choose Advanced: coins, fee and change ▸ Fully mixed funds only (CoinJoin).";

for (const theme of THEMES) {
  test(`UNMIXED a normal payment while mixing says it is not private, ${theme}`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await setUp(page, theme, "mixing", 2);
    await page.getByTestId("pay-amount").fill("0.2");
    await review(page);
    await expect(page.getByTestId("pay-confirm-unmixed")).toHaveText(NOTE);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await fontsReady(page);
    await page.getByTestId("pay-confirm").screenshot({ path: artifactPath(info, `${theme}-pay-confirm-unmixed.png`) });
    expect(errors).toEqual([]);
  });
}

test("UNMIXED nothing is said in a wallet that does not mix, or for a fully mixed payment", async ({ page }) => {
  await setUp(page, "light", "idle", 0);
  await page.getByTestId("pay-amount").fill("0.2");
  await review(page);
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
  await expect(page.getByTestId("pay-confirm-unmixed")).toHaveCount(0);

  await setUp(page, "light", "mixing", 2);
  await page.getByTestId("pay-advanced").locator("summary").click();
  await page.getByTestId("coin-choice-mixed").check();
  await page.getByTestId("pay-amount").fill("0.05");
  await review(page);
  await expect(page.getByTestId("pay-confirm-unmixed")).toHaveCount(0);
});

// Coin control spends the send account's coins: a BIP44 denomination dash-qt mixed is one.
test("UNMIXED selected coins: silent for a mixed denomination alone, said once an unmixed coin is in", async ({ page }) => {
  await setUp(page, "light", "mixing", 1);
  await page.evaluate(() => {
    const w = window.__dwFixture!.world;
    const mixed = w.wallets[0].coins[1];
    w.coinjoin.restoredOutpoints = [`${mixed.outpoint.txid}:${mixed.outpoint.vout}`];
  });
  await page.getByTestId("pay-amount").fill("0.05");
  await page.getByTestId("pay-advanced").locator("summary").click();
  await page.getByTestId("coin-choice-selected").check();
  await page.getByRole("checkbox", { name: /^Use 0\.100001 / }).check();
  await review(page);
  await expect(page.getByTestId("pay-confirm-unmixed")).toHaveCount(0);
  await page.getByTestId("pay-confirm-cancel").click();
  await page.getByRole("checkbox", { name: /^Use 0\.50/ }).check();
  await review(page);
  await expect(page.getByTestId("pay-confirm-unmixed")).toHaveText(NOTE);
});
