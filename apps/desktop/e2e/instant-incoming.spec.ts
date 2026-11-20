// Instant incoming payments (PRODUCT.md #1): a payment that arrives while the wallet still syncs
// shows on Home and in Activity with the balance it makes, and Home says the balance may change
// until this wallet is scanned to the tip.

import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

const ROW = "tx-row-5f0c2a6e9d-0";
const SYNCING = "Still syncing, balance may change";
const STOPPED = "Syncing has stopped, balance may be out of date";

async function checked(page: Page, errors: string[]) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
}

async function openHome(page: Page, scenario: string, theme: "light" | "dark") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  return errors;
}

async function expectLine(page: Page, text: string) {
  await expect(page.getByTestId("hero-sync-line")).toHaveText(text, { timeout: 15_000 });
  await expect(page.getByRole("status").filter({ hasText: text })).toHaveCount(1);
}

for (const theme of THEMES) {
  for (const [scenario, chip] of [
    ["catching-up-payment", "InstantSend"],
    ["catching-up-pending-payment", "Pending"],
  ] as const) {
    test(`${scenario} (${theme}): Home and Activity show the payment and its balance while syncing`, async ({ page }) => {
      const errors = await openHome(page, scenario, theme);
      const hero = page.getByTestId("hero");
      await expect(hero.getByTestId("hero-amount")).toContainText("26.65", { timeout: 15_000 });
      await expect(hero.getByTestId("breakdown-core")).toContainText("25.00");
      await expect(hero).not.toContainText("Balance unavailable");
      await expectLine(page, SYNCING);
      const homeRow = page.getByTestId(ROW);
      await expect(homeRow).toContainText("Received");
      await expect(homeRow).toContainText("+25.00");
      await expect(homeRow).toContainText(chip);
      await checked(page, errors);

      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      const row = page.getByTestId(ROW);
      await expect(row).toContainText("+25.00");
      await expect(row).toContainText(chip);
      await checked(page, errors);
    });
  }

  test(`a synced wallet's Home has no sync line (${theme})`, async ({ page }) => {
    const errors = await openHome(page, "funded", theme);
    await expect(page.getByTestId("hero-sync-line")).toHaveText("");
    await expect(page.getByTestId("hero-amount")).not.toContainText("Balance unavailable");
    await checked(page, errors);
  });

  for (const [scenario, line, amount, history] of [
    ["wallet-behind-stopped", STOPPED, "32.04", null],
    ["wallet-behind-stopped-empty", STOPPED, "0.00", "Syncing has stopped"],
    ["wallet-behind-at-tip", SYNCING, "32.04", null],
    ["wallet-behind-at-tip-empty", SYNCING, "0.00", "Your history is still syncing"],
  ] as const) {
    test(`${scenario} (${theme}): a wallet behind the tip keeps a caveat on its balance`, async ({ page }) => {
      const errors = await openHome(page, scenario, theme);
      await expect(page.getByTestId("hero-amount")).toContainText(amount, { timeout: 15_000 });
      await expectLine(page, line);
      if (history) await expect(page.getByTestId("history-syncing")).toContainText(history);
      await checked(page, errors);
    });
  }
}

test("a payment delivered while Home is open updates the balance and the row in place", async ({ page }) => {
  const errors = await openHome(page, "catching-up-nothing-yet", "light");
  const hero = page.getByTestId("hero");
  await expect(hero.getByTestId("breakdown-core")).toContainText("0.00", { timeout: 15_000 });
  await expectLine(page, SYNCING);
  await expect(page.getByTestId(ROW)).toHaveCount(0);
  const url = page.url();
  await page.evaluate(() => ((window as unknown as { __samePage: boolean }).__samePage = true));

  await page.evaluate(() => {
    const fx = window.__dwFixture!;
    const main = fx.world.wallets[0];
    const network = fx.world.launch.network.dirName;
    const walletId = main.summary.walletId;
    const paid = 25 * 100_000_000;
    const txid = "5f0c2a6e9d4b4a3c8e7f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d";
    main.rows = [
      {
        txid,
        recordIndex: 0,
        txType: "recvWithAddress",
        category: "received",
        status: { kind: "unconfirmed", confirmations: 0, instantLocked: true, chainLocked: false, maturesIn: null },
        timestamp: Math.floor(Number(fx.now) / 1000),
        blockHeight: null,
        amount: paid,
        fee: null,
        address: "yXbH3JxbVWgEzq6aZDyEbNQx5PtUfZvvXZ",
        label: null,
        countsTowardBalance: true,
        involvesWatchOnly: false,
      },
    ];
    const balances = { confirmed: paid, unconfirmed: 0, immature: 0, locked: 0, total: paid, coinjoin: 0, catchingUp: true };
    main.summary = { ...main.summary, balances };
    fx.emit({ kind: "newTransactions", network, walletId, txids: [txid], count: 1, catchUp: true });
    fx.emit({ kind: "balances", network, walletId, balances });
    fx.emit({ kind: "historyChanged", network, walletId, txids: [txid] });
  });

  await expect(hero.getByTestId("breakdown-core")).toContainText("25.00");
  await expect(hero.getByTestId("hero-amount")).toContainText("26.65");
  const row = page.getByTestId(ROW);
  await expect(row).toContainText("+25.00");
  await expect(row).toContainText("InstantSend");
  await expectLine(page, SYNCING);
  expect(page.url()).toBe(url);
  expect(await page.evaluate(() => (window as unknown as { __samePage?: boolean }).__samePage)).toBe(true);
  await checked(page, errors);
});
