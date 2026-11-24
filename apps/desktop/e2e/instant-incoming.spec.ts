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
    const balances = { confirmed: paid, unconfirmed: 0, immature: 0, locked: 0, total: paid, coinjoin: 0, catchingUp: true, heldUnfinished: 0, heldUnvalued: 0 };
    main.summary = { ...main.summary, balances };
    fx.emit({ kind: "newTransactions", network, walletId, txids: [txid], live: [true], count: 1, catchUp: true });
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

test("a new block on a synced wallet does not touch the hero line while the scan catches up", async ({ page }) => {
  const errors = await openHome(page, "funded", "light");
  await expect(page.getByTestId("hero-amount")).toContainText("32.04", { timeout: 15_000 });
  const line = page.getByTestId("hero-sync-line");
  await expect(line).toHaveText("");
  await page.evaluate(() => {
    const el = document.querySelector('[data-testid="hero-sync-line"]')!;
    const seen: string[] = [];
    (window as unknown as { __heroLines: string[] }).__heroLines = seen;
    new MutationObserver(() => seen.push(el.textContent ?? "")).observe(el, { childList: true, characterData: true, subtree: true });
  });
  const seen = () => page.evaluate(() => (window as unknown as { __heroLines: string[] }).__heroLines);

  const tip = await page.evaluate(() => {
    const fx = window.__dwFixture!;
    const old = fx.world.sync.tipHeight!;
    const next = old + 1;
    fx.world.wallets[0].scanHeight = old;
    fx.world.sync = {
      ...fx.world.sync,
      tipHeight: next,
      caughtUp: false,
      activePhase: "filters",
      phases: fx.world.sync.phases.map((p) => ({ ...p, targetHeight: next, currentHeight: p.phase === "headers" ? next : old, done: p.phase === "headers" })),
    };
    fx.emit({ kind: "sync", network: fx.world.launch.network.dirName, status: fx.world.sync });
    return next;
  });
  await expect
    .poll(() => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "wallet_scan_height").length), { timeout: 15_000 })
    .toBeGreaterThan(1);
  await page.waitForTimeout(1500);
  await expect(line).toHaveText("");

  await page.evaluate((next) => {
    const fx = window.__dwFixture!;
    fx.world.wallets[0].scanHeight = next;
    fx.world.sync = { ...fx.world.sync, caughtUp: true, activePhase: null, phases: fx.world.sync.phases.map((p) => ({ ...p, currentHeight: next, done: true })) };
    fx.emit({ kind: "sync", network: fx.world.launch.network.dirName, status: fx.world.sync });
  }, tip);
  await page.waitForTimeout(1500);
  await expect(line).toHaveText("");
  expect((await seen()).filter((t) => t !== "")).toEqual([]);
  await checked(page, errors);
});

test("a synced wallet's hero is empty for 2 s while its first scan-height read is out, then the caveat, then empty once it returns", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.addInitScript(() => {
    const seen: { text: string; at: number }[] = [];
    (window as unknown as { __heroLines: typeof seen }).__heroLines = seen;
    const read = () => {
      const el = document.querySelector('[data-testid="hero-sync-line"]');
      if (!el) return;
      const text = el.textContent ?? "";
      if (seen.length === 0 || seen[seen.length - 1].text !== text) seen.push({ text, at: performance.now() });
    };
    new MutationObserver(read).observe(document, { childList: true, characterData: true, subtree: true });
  });
  const lines = () => page.evaluate(() => (window as unknown as { __heroLines: { text: string; at: number }[] }).__heroLines);
  const scanReads = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "wallet_scan_height").length);

  await page.goto(fixtureUrl("funded", "light", "&hold=wallet_scan_height"));
  await expect(page.getByTestId("hero-amount")).toContainText("32.04", { timeout: 15_000 });
  expect(await scanReads()).toBe(1);
  await expectLine(page, SYNCING);
  expect(await scanReads()).toBe(1);
  const pending = await lines();
  expect(pending.map((l) => l.text)).toEqual(["", SYNCING]);
  expect(pending[1].at - pending[0].at).toBeGreaterThanOrEqual(1_500);

  await page.evaluate(() => window.__dwFixture!.release("wallet_scan_height"));
  await expect(page.getByTestId("hero-sync-line")).toHaveText("", { timeout: 15_000 });
  expect((await lines()).map((l) => l.text)).toEqual(["", SYNCING, ""]);
  expect(errors).toEqual([]);
});
