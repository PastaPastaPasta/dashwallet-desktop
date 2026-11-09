import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE, WALLET_VIEWS, showWalletView } from "./wallet";

// U-03: Home's shortcuts, Pay ▸ To an address (estimate, refusals, coin control, confirm with
// the payment policy, the result), Receive (address, QR, requests) and Activity (table mode,
// filters, label, abandon) on the fixture backend. The fixture's send engine is src/fixture/send.ts.

async function open(page: Page, scenario = "funded", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const world = <T>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function fillPayment(page: Page, address: string, amount: string) {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  await page.getByTestId("pay-address").fill(address);
  await page.getByTestId("pay-amount").fill(amount);
}

/** Normal coins 0.1 and 0.4, a fully mixed 0.100001, and a mixer that has some of each; the cached status is refreshed as the engine's event does. */
async function seedMixing(page: Page) {
  await world(page, (w) => {
    const wallet = w.wallets[0];
    const c = wallet.coins[0];
    const coin = (vout: number, amount: number, extra: object) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount, spendable: true, confirmations: 10, ...extra });
    wallet.coins = [
      coin(1, 10_000_000, { coinjoinDenominated: false, coinjoinRounds: null }),
      coin(2, 40_000_000, { coinjoinDenominated: false, coinjoinRounds: null }),
      coin(3, 10_000_100, { coinjoinDenominated: true, coinjoinRounds: 4 }),
    ];
    w.coinjoin.mixers[wallet.summary.walletId] = {
      state: "idle",
      stopReason: null,
      status: { kind: "idle" },
      progress: { overallPercent: 40, denominatedPercent: 100, partiallyMixedPercent: 60, mixedPercent: 10, averageRounds: 1.5 },
      sessions: [],
      queueSize: 0,
    };
  });
  await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId }); })()`);
}

/** One unmixed coin of 215.74478465 and a coin still being mixed (no fully mixed ones): a normal
 * payment of everything with Subtract fee leaves it nothing (ROW73, pasta's 11-08 screenshot). */
async function seedEmptiedByPayment(page: Page) {
  await world(page, (w) => {
    const wallet = w.wallets[0];
    const c = wallet.coins[0];
    const coin = (vout: number, amount: number, extra: object) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount, spendable: true, confirmations: 10, ...extra });
    wallet.coins = [coin(1, 21_574_478_465, { coinjoinDenominated: false, coinjoinRounds: null }), coin(2, 100_001_000, { coinjoinDenominated: true, coinjoinRounds: 1 })];
    // The fixture's mixing figures are shares of the confirmed balance: make it these coins.
    const b = wallet.summary.balances!;
    const confirmed = 21_574_478_465 + 100_001_000;
    wallet.summary = { ...wallet.summary, balances: { ...b, confirmed, total: b.total - b.confirmed + confirmed } };
    w.coinjoin.mixers[wallet.summary.walletId] = {
      state: "idle",
      stopReason: null,
      status: { kind: "idle" },
      progress: { overallPercent: 20, denominatedPercent: 100, partiallyMixedPercent: 100, mixedPercent: 0, averageRounds: 1 },
      sessions: [],
      queueSize: 0,
    };
  });
  await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId }); })()`);
}

/** Waits until the app called the backend after `command` and then made no call for 750 ms. */
async function settledAfter(page: Page, command: string) {
  const calls = () => page.evaluate(() => [...window.__dwFixture!.calls]);
  const at = (await calls()).lastIndexOf(command);
  expect(at).toBeGreaterThanOrEqual(0);
  await expect.poll(async () => (await calls()).length).toBeGreaterThan(at + 1);
  let last = -1;
  await expect
    .poll(
      async () => {
        const n = (await calls()).length;
        const quiet = n === last;
        last = n;
        return quiet;
      },
      { intervals: [750], timeout: 15_000 },
    )
    .toBe(true);
}

async function review(page: Page) {
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
}

async function sendWithPassphrase(page: Page, passphrase = "demo") {
  await page.getByTestId("pay-confirm-passphrase").fill(passphrase);
  await page.getByTestId("pay-confirm-send").click();
}

test.describe("Home", () => {
  test("the shortcuts open Receive and Pay; Back up says why it is off", async ({ page }) => {
    await open(page);
    await page.getByTestId("shortcut-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
    await page.getByTestId("sidebar-home").click();
    await page.getByTestId("shortcut-send").click();
    await expect(page.getByTestId("pay-address-form")).toBeVisible();
    await page.getByTestId("sidebar-home").click();
    // Scan QR: no camera on the desktop, so the address field takes a pasted code.
    await page.getByTestId("shortcut-scan").click();
    await expect(page.getByTestId("pay-address")).toBeFocused();
    await page.getByTestId("sidebar-home").click();
    const backup = page.getByTestId("shortcut-backup");
    await expect(backup).toHaveAttribute("aria-disabled", "true");
    await expect(backup).toHaveAttribute("title", /not available yet/);
    // Playwright waits for an aria-disabled button to enable; the click is the user's anyway.
    await backup.click({ force: true });
    await expect(page.getByTestId("page-home")).toBeVisible();
  });

  test("an empty wallet offers Receive", async ({ page }) => {
    await open(page, "empty");
    await page.getByTestId("history-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });
});

test.describe("Pay", () => {
  test("estimates the fee and refuses as the engine does, on the field", async ({ page }) => {
    const errors = await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await expect(page.getByTestId("pay-fee-line")).toContainText("0.00000226");
    await expect(page.getByTestId("pay-continue")).toBeEnabled();

    await page.getByTestId("pay-amount").fill("0.000001");
    await expect(page.locator(".field-error", { hasText: "below the dust threshold" })).toBeVisible();
    await expect(page.getByTestId("pay-problem")).toContainText("below the dust threshold");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();

    await page.getByTestId("pay-amount").fill("500");
    await expect(page.locator(".field-error", { hasText: "The amount exceeds your balance." })).toBeVisible();

    await page.getByTestId("pay-amount").fill("1.2.3");
    await expect(page.getByText("Enter an amount in DASH")).toBeVisible();

    await page.getByTestId("pay-amount").fill("0.5");
    await page.getByTestId("pay-address").fill("not an address");
    await expect(page.getByText("The recipient address is not valid. Please recheck.")).toBeVisible();
    await page.getByTestId("pay-address").fill("XnWbGbBDpTK8GvWLDZqpuqZQELVeCkqnmb");
    await expect(page.getByText("This address is not valid on this network.")).toBeVisible();
    await expect(page.getByTestId("pay-address")).toHaveAttribute("aria-invalid", "true");
    expect(errors).toEqual([]);
  });

  test("Max pays everything spendable with the fee taken from it", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "");
    await page.getByTestId("pay-max").click();
    // dash-qt's "Use available balance": the four confirmed coins, the fee taken from them
    // (10 + 4 × 148 + 34 = 636 bytes at 1 000 duffs/kB, no change).
    await expect(page.getByTestId("pay-amount")).toHaveValue("30.19698287");
    await expect(page.getByTestId("pay-subtract-fee")).toBeChecked();
    await review(page);
    await expect(page.getByTestId("pay-confirm-amount")).toContainText("30.19697651");
    await expect(page.getByTestId("pay-confirm-fee")).toContainText("0.00000636");
    await expect(page.getByTestId("pay-confirm-total")).toContainText("30.19698287");
  });

  test("sends with the passphrase, shows the result and the transaction", async ({ page }) => {
    const errors = await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await page.getByTestId("pay-label").fill("Alice");
    await review(page);
    await expect(page.getByTestId("pay-confirm-address")).toContainText("Alice");
    await expect(page.getByTestId("pay-confirm-address")).toContainText(PAYEE);
    await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.50000000");
    await expect(page.getByTestId("pay-confirm-fee")).toContainText("0.00000226");
    await expect(page.getByTestId("pay-confirm-total")).toContainText("0.50000226");
    await expect(page.getByTestId("pay-confirm-passphrase")).toBeFocused();
    // The shell behind the sheet takes no focus (U-03 D22).
    await expect(page.locator(".app-chrome")).toHaveAttribute("inert", "");

    // A wrong passphrase is refused and cleared, and the field has the focus again; nothing is signed.
    await sendWithPassphrase(page, "wrong");
    await expect(page.getByTestId("pay-confirm-error")).toHaveText("The passphrase you entered was incorrect.");
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveValue("");
    await expect(page.getByTestId("pay-confirm-passphrase")).toBeFocused();
    expect(await world(page, (w) => Object.keys(w.send.prepared).length)).toBe(0);

    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    const txid = await world(page, (w) => w.wallets[0].rows[0].txid);
    await expect(page.getByTestId("pay-result-txid")).toContainText(txid);
    // The spend grant was capped at the confirmed total.
    expect(await world(page, (w) => w.grants.map((g) => [g.grant.purpose, g.used]))).toEqual([[{ kind: "spend", maxDuffs: 50_000_226 }, true]]);

    await page.getByTestId("pay-result-show").click();
    await expect(page.getByTestId("page-activity")).toBeVisible();
    await expect(page.getByTestId("tx-detail-facts")).toContainText(txid);
    await page.keyboard.press("Escape");
    // The form starts over for the next payment, and the balance moved.
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByTestId("pay-address")).toHaveValue("");
    await expect(page.getByTestId("pay-address-form")).toContainText("Available: 29.69698061");
    expect(errors).toEqual([]);
  });

  test("Send waits out the vault's throttle after a wrong passphrase", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.throttleSecs = 3));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await sendWithPassphrase(page, "wrong");
    await expect(page.getByTestId("pay-confirm-error")).toHaveText("The passphrase you entered was incorrect.");
    await expect(page.getByTestId("pay-confirm-wait")).toHaveText(/^Try again in [1-3] s\.$/);
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    // Typing is fine meanwhile; Return does not send either.
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await page.getByTestId("pay-confirm-passphrase").press("Enter");
    expect(await world(page, (w) => w.grants.length)).toBe(0);
    // Once the wait is over, Send works.
    await expect(page.getByTestId("pay-confirm-send")).toBeEnabled({ timeout: 10_000 });
    await expect(page.getByTestId("pay-confirm-wait")).toHaveCount(0);
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  });

  test("a confirm sheet opened during the vault's wait holds Send too", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.throttleSecs = 4));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await sendWithPassphrase(page, "wrong");
    await expect(page.getByTestId("pay-confirm-wait")).toBeVisible();
    await page.getByTestId("pay-confirm-cancel").click();
    await review(page);
    await expect(page.getByTestId("pay-confirm-wait")).toHaveText(/^Too many attempts\. Try again in [1-4] s\.$/);
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    await expect(page.getByTestId("pay-confirm-send")).toBeEnabled({ timeout: 10_000 });
    await sendWithPassphrase(page, "demo");
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  });

  test("Send waits for the sheet's own read of the vault's wait (Sol r2)", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    // A wait the vault reports, and its answer late.
    await world(page, (w) => (w.throttledUntil = Date.now() + 4_000));
    await page.evaluate(() => {
      (window as unknown as { release: () => void }).release = window.__dwFixture!.hold("lock_status");
    });
    await review(page);
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    await expect(page.getByTestId("pay-confirm-wait")).toHaveCount(0);
    await page.getByTestId("pay-confirm-passphrase").press("Enter");
    expect(await world(page, (w) => w.grants.length)).toBe(0);
    await page.evaluate(() => (window as unknown as { release: () => void }).release());
    await expect(page.getByTestId("pay-confirm-wait")).toHaveText(/^Too many attempts\. Try again in [1-4] s\.$/);
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    await expect(page.getByTestId("pay-confirm-send")).toBeEnabled({ timeout: 10_000 });
  });

  test("a failed read of the vault's wait keeps Send off and says so", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await page.evaluate(() => window.__dwFixture!.fail("lock_status", "none"));
    await review(page);
    await expect(page.getByTestId("pay-confirm-error")).not.toBeEmpty();
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    await page.getByTestId("pay-confirm-passphrase").press("Enter");
    expect(await world(page, (w) => w.grants.length)).toBe(0);
  });

  test("with the payment setting off, the unlocked vault signs and Cancel has the focus", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.paymentAuth = false));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await expect(page.getByTestId("pay-confirm-cancel")).toBeFocused();
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
    // Return on Cancel does not send; focus goes back to Review.
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
    await expect(page.getByTestId("pay-continue")).toBeFocused();
    await review(page);
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  });

  test("the engine asking for authentication brings the passphrase back", async ({ page }) => {
    await open(page);
    // The page read the setting as off, but the engine has it on (it changed meanwhile).
    await world(page, (w) => (w.paymentAuth = false));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await world(page, (w) => (w.paymentAuth = true));
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-confirm-passphrase")).toBeVisible();
    expect(await world(page, (w) => w.wallets[0].rows[0].amount)).toBe(20_000_000);
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  });

  test("an unknown broadcast keeps the transaction for Try again", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.send.broadcast = "unknown"));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
    await expect(page.getByTestId("pay-result-text")).toContainText("Check Activity before you pay again.");
    await expect(page.getByTestId("pay-result-retry")).toBeFocused();
    // A payment that may have gone ends the form at once; the draft stays open for Try again.
    await expect(page.getByTestId("pay-address")).toHaveValue("");
    await expect(page.getByTestId("pay-amount")).toHaveValue("");
    // A retry that fails is still unknown, never "not sent" (U-03 D10): the user must not pay twice.
    // (The engine answers `broadcast_unknown` for a handed-over transaction whatever went wrong.)
    await world(page, (w) => (w.send.broadcast = "noPeers"));
    await page.getByTestId("pay-result-retry").click();
    await expect(page.getByTestId("pay-result-retry")).toHaveText("Try again");
    await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
    await expect(page.getByTestId("pay-result-failed")).toHaveCount(0);
    await expect(page.getByTestId("pay-result-retry")).toBeFocused();
    expect(await world(page, (w) => w.wallets[0].coins.some((u) => u.reserved))).toBe(false);
    await world(page, (w) => (w.send.broadcast = "ok"));
    await page.getByTestId("pay-result-retry").click();
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    // Recorded once, when it was handed over (the fixture history already had one such row).
    expect(await world(page, (w) => w.wallets[0].rows.filter((r) => r.amount === -50_000_226).length)).toBe(2);
  });

  test("a signed transaction that differs from the sheet is abandoned, never sent (D23)", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    // The coins change after Review: the same fee and total, from another coin, with other change.
    await world(page, (w) => (w.wallets[0].coins = w.wallets[0].coins.filter((u) => u.amount !== 2_000_000_000)));
    const rows = await world(page, (w) => w.wallets[0].rows.length);
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-failed")).toBeVisible();
    await expect(page.getByTestId("pay-result-text")).toHaveText("The payment changed. Review it again.");
    expect(await world(page, (w) => [w.wallets[0].rows.length, Object.keys(w.send.prepared).length, w.wallets[0].coins.some((u) => u.reserved)])).toEqual([
      rows,
      0,
      false,
    ]);
  });

  test("a rejected broadcast releases the coins and keeps the form", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.send.broadcast = "rejected"));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-failed")).toBeVisible();
    await expect(page.getByTestId("pay-result-text")).toContainText("The network rejected the transaction: bad-txns-inputs-missingorspent.");
    await page.getByTestId("pay-result-back").click();
    await expect(page.getByTestId("pay-amount")).toHaveValue("0.5");
    // The result replaced the confirm sheet; closing it still returns focus to the form.
    await expect(page.getByTestId("pay-continue")).toBeFocused();
    expect(await world(page, (w) => w.wallets[0].coins.some((u) => u.reserved))).toBe(false);
  });

  test("coin control: selected coins, mixed funds, a custom fee rate, a change address", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await page.getByTestId("pay-advanced").locator("summary").click();

    await page.getByTestId("coin-choice-mixed").check();
    await expect(page.getByTestId("pay-amount")).toHaveAttribute("aria-invalid", "true");
    await expect(page.locator(".field-error", { hasText: /^Not enough fully mixed funds: 0\.00.* available\.$/ })).toBeVisible();

    await page.getByTestId("coin-choice-selected").check();
    await expect(page.getByTestId("pay-problem")).toHaveText("Choose the coins to pay with.");
    const rows = page.getByTestId("coin-table").locator("tbody tr");
    await expect(rows).toHaveCount(5);
    // The unconfirmed coin cannot be chosen.
    await expect(rows.nth(4).locator("input")).toBeDisabled();
    await rows.nth(3).locator("input").check();
    await expect(page.getByTestId("coin-insufficient")).toBeVisible();
    await rows.nth(2).locator("input").check();
    await expect(page.getByTestId("coin-summary")).toContainText("2.69698287");
    await expect(page.getByTestId("pay-fee-line")).toContainText("0.00000374");

    await page.getByTestId("fee-custom").check();
    await expect(page.getByTestId("pay-problem")).toHaveText("Enter a custom fee rate.");
    await page.getByTestId("fee-rate").fill("0.000001");
    await expect(page.getByText("The fee rate must be at least 0.00001")).toBeVisible();
    await page.getByTestId("fee-rate").fill("0.0001");
    await expect(page.getByTestId("pay-fee-line")).toContainText("0.0000374");

    await page.getByTestId("change-custom").check();
    await page.getByTestId("change-address").fill("bad");
    await expect(page.getByTestId("pay-problem")).toHaveText("The custom change address is not valid.");
    await page.getByTestId("change-address").fill(PAYEE.replace("Cd", "Ce"));
    await review(page);
    await expect(page.getByTestId("pay-confirm-facts")).toContainText("2 selected coins");
    // Change to an address the wallet may not own counts toward the total and the grant (D21).
    await expect(page.getByTestId("pay-confirm-change")).toContainText(PAYEE.replace("Cd", "Ce"));
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    const [cap, debit] = await world(page, (w) => [w.grants[0].grant.purpose, -w.wallets[0].rows[0].amount]);
    expect(cap).toEqual({ kind: "spend", maxDuffs: debit });
  });

  test("a fully mixed payment refused as an absurd fee offers Subtract fee (MX-FEE)", async ({ page }) => {
    await open(page);
    // Re-smoke 2 §3c: two fully mixed 0.100001 coins; 0.1 without Subtract fee leaves 0.100002 as fee.
    await world(page, (w) => {
      const c = w.wallets[0].coins[0];
      w.wallets[0].coins.push(...[91, 92].map((vout) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount: 10_000_100, coinjoinRounds: 4 })));
    });
    await fillPayment(page, PAYEE, "0.1");
    await page.getByTestId("pay-advanced").locator("summary").click();
    await page.getByTestId("coin-choice-mixed").check();
    await expect(page.getByTestId("pay-problem")).toHaveText(
      "Fully mixed coins are spent in whole denominations, so the 0.10000200 tDASH left over would go to fees.",
    );
    await expect(page.getByTestId("pay-offer")).toHaveText(/^\s*With Subtract fee it would send 0\.09999908 tDASH\./);
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
    // Review's description is the reason alone, not the offer (r1 Opus nit 2), and the state passes axe.
    await expect(page.getByTestId("pay-continue")).toHaveAccessibleDescription(/^Fully mixed coins .* would go to fees\.$/);
    expect(await seriousAxeViolations(page)).toEqual([]);
    // While a changed amount is still being read, the old quote is not offered (r1 Sol minor).
    await page.getByTestId("pay-amount").fill("0.05");
    expect(await page.getByTestId("pay-subtract-offer").count()).toBe(0);
    await page.getByTestId("pay-amount").fill("0.1");
    await page.getByTestId("pay-subtract-offer").click();
    await expect(page.getByTestId("pay-subtract-fee")).toBeChecked();
    await expect(page.getByTestId("pay-fee-line")).toContainText("0.00000192");
    await review(page);

    // When Subtract fee is refused too (one 1.00001 coin paying 0.5 burns the rest either way), only the reason.
    await page.getByTestId("pay-confirm-cancel").click();
    await world(page, (w) => {
      w.wallets[0].coins = w.wallets[0].coins.filter((u) => u.outpoint.vout !== 92);
      w.wallets[0].coins.find((u) => u.outpoint.vout === 91)!.amount = 100_001_000;
    });
    await page.getByTestId("pay-amount").fill("0.5");
    await page.getByTestId("pay-subtract-fee").uncheck();
    await expect(page.getByTestId("pay-problem")).toHaveText("Fully mixed coins are spent in whole denominations, so the 0.50001000 tDASH left over would go to fees.");
    await expect(page.getByTestId("pay-subtract-offer")).toHaveCount(0);
  });

  test("Available counts what the coin source can spend, and the over-balance refusal says so (PAY-MIX)", async ({ page }) => {
    await open(page);
    // Re-smoke 2 §3b on W1: unmixed 0.5, partly mixed 1.00001 (mixing), fully mixed 0.100001.
    await world(page, (w) => {
      const wallet = w.wallets[0];
      const c = wallet.coins[0];
      const coin = (vout: number, amount: number, extra: object) => ({ ...c, outpoint: { ...c.outpoint, vout }, amount, spendable: true, confirmations: 10, ...extra });
      wallet.coins = [
        coin(1, 50_000_000, { coinjoinDenominated: false, coinjoinRounds: null }),
        coin(2, 100_001_000, { coinjoinDenominated: true, coinjoinRounds: 1 }),
        coin(3, 10_000_100, { coinjoinDenominated: true, coinjoinRounds: 4 }),
      ];
      w.coinjoin.mixers[wallet.summary.walletId] = {
        state: "idle",
        stopReason: null,
        status: { kind: "idle" },
        progress: { overallPercent: 40, denominatedPercent: 100, partiallyMixedPercent: 60, mixedPercent: 10, averageRounds: 1.5 },
        sessions: [],
        queueSize: 0,
      };
    });
    // The mixer changed under Home's cached status: say so, as the engine's event does.
    await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId }); })()`);
    await fillPayment(page, PAYEE, "0.8");
    // The normal source: unmixed coins only (not the 1.60001100 the wallet holds).
    await expect(page.getByText("Available: 0.50000000 tDASH")).toBeVisible();
    const reason =
      "The amount exceeds the 0.50000000 tDASH a normal payment can spend. To spend fully mixed coins, choose Advanced: coins, fee and change ▸ Fully mixed funds only (CoinJoin). Coins still being mixed can be spent that way once mixing finishes.";
    await expect(page.locator(".field-error")).toHaveText(reason);
    // The footer says why Review is off, instead of "Checking the payment…".
    await expect(page.getByTestId("pay-problem")).toHaveText(reason);
    await expect(page.getByTestId("pay-incomplete")).toHaveCount(0);
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
    await expect(page.getByTestId("pay-continue")).toHaveAccessibleDescription(reason);
    expect(await seriousAxeViolations(page)).toEqual([]);

    await page.getByTestId("pay-advanced").locator("summary").click();
    await page.getByTestId("coin-choice-mixed").check();
    await expect(page.getByTestId("pay-problem")).toHaveText("Not enough fully mixed funds: 0.10000100 tDASH available.");
    // A refusal replaces the hint under the amount; within the funds it is back.
    await page.getByTestId("pay-amount").fill("0.05");
    await expect(page.getByText("Available (fully mixed): 0.10000100 tDASH")).toBeVisible();

    await page.getByTestId("coin-choice-selected").check();
    await expect(page.getByText(/^Available/)).toHaveCount(0);
    await page.getByRole("checkbox", { name: /^Use 0\.50 / }).check();
    await expect(page.getByText("Available (selected coins): 0.50000000 tDASH")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    // A CoinJoin coin is no Selected coin for the engine (`send.outpoint_unavailable`): no figure, the reason.
    await page.getByRole("checkbox", { name: /^Use 0\.50 / }).uncheck();
    await page.getByRole("checkbox", { name: /^Use 0\.100001 / }).check();
    await expect(page.getByTestId("pay-problem")).toHaveText("A selected coin is spent, locked or no longer in the wallet. Choose the coins again.");
    await expect(page.getByText(/^Available/)).toHaveCount(0);

  });

  test("a refusal under Selected coins gives no normal-payment or mixing advice (PAY-MIX r1)", async ({ page }) => {
    await open(page);
    await seedMixing(page);
    await fillPayment(page, PAYEE, "0.2");
    await page.getByTestId("pay-advanced").locator("summary").click();
    // 0.5 of normal coins covers 0.2, so the status stays cached with its hints.
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await page.getByTestId("coin-choice-selected").check();
    await page.getByRole("checkbox", { name: /^Use 0\.10 / }).check();
    const plain = "The amount exceeds your balance.";
    await expect(page.locator(".field-error")).toHaveText(plain);
    await expect(page.getByTestId("pay-problem")).toHaveText(plain);
  });

  test("an empty custom change address holds the payment", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await page.getByTestId("pay-advanced").locator("summary").click();
    await page.getByTestId("change-custom").check();
    await expect(page.getByTestId("pay-problem")).toHaveText("Enter the custom change address.");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
  });

  test("Cancel on the confirm sheet returns to Review, ready (ROW73)", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await page.getByTestId("pay-confirm-cancel").click();
    await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await expect(page.getByTestId("pay-continue")).toBeFocused();
  });

  test("coins that change while the confirm sheet is up re-check nothing behind it, and Cancel checks again (ROW73)", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    const form = page.getByTestId("pay-address-form");
    const available = form.getByText(/^Available: /);
    await expect(available).toBeVisible();
    const before = await available.textContent();
    await review(page);
    // Every coin goes (another device spent them), and the engine says so.
    await page.evaluate(`(() => {
      const f = window.__dwFixture;
      const w = f.world.wallets[0];
      for (const u of w.coins) u.spendable = false;
      f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.summary.walletId, balances: w.summary.balances });
    })()`);
    const calls = () => page.evaluate(() => window.__dwFixture!.calls.length);
    let last = -1;
    await expect
      .poll(
        async () => {
          const n = await calls();
          const quiet = n === last;
          last = n;
          return quiet;
        },
        { intervals: [750], timeout: 15_000 },
      )
      .toBe(true);
    await expect(form.locator(".field-error")).toHaveCount(0);
    await expect(page.getByTestId("pay-problem")).toHaveCount(0);
    await expect(available).toHaveText(before!);
    await page.getByTestId("pay-confirm-cancel").click();
    await expect(page.getByTestId("pay-problem")).toContainText("The amount exceeds");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
  });

  test("Send writes Confirm's payment into the draft before signing, whatever the draft held (ROW73)", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    // Something writes another amount into the open draft while Confirm is up.
    const tampered = await page.evaluate(async () => {
      const f = window.__dwFixture!;
      const [id, d] = Object.entries(f.world.send.drafts).find(([, d]) => d.recipients[0]?.amount === 50_000_000)!;
      const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke(c: string, a: unknown): Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
      await invoke("draft_set_recipients", { draft: Number(id), recipients: [{ ...d.recipients[0], amount: 20_000_000 }] });
      return { id: Number(id), from: f.calls.length };
    });
    expect(await page.evaluate((id) => window.__dwFixture!.world.send.drafts[id]?.recipients[0]?.amount, tampered.id)).toBe(20_000_000);
    await sendWithPassphrase(page);
    // What prepare signed, checked before Confirm compares the summary with what it showed.
    const after = () => page.evaluate((from) => window.__dwFixture!.calls.slice(from), tampered.from);
    await expect.poll(async () => (await after()).includes("draft_prepare")).toBe(true);
    const calls = await after();
    const rewrite = calls.indexOf("draft_set_recipients");
    expect(rewrite).toBeGreaterThanOrEqual(0);
    expect(calls.indexOf("draft_prepare")).toBeGreaterThan(rewrite);
    expect(await page.evaluate((id) => window.__dwFixture!.world.send.drafts[id]?.recipients[0]?.amount, tampered.id)).toBe(50_000_000);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    await expect(page.getByTestId("pay-result")).toContainText("0.50000000");
  });

  test("a payment of all the normal funds shows no refusal behind its result, and Close starts a new payment (ROW73)", async ({ page }) => {
    await open(page);
    // Pasta's 11-08 wallet: one unmixed coin, and coins still being mixed (none fully mixed yet).
    await seedEmptiedByPayment(page);
    await fillPayment(page, PAYEE, "215.74478465");
    await page.getByTestId("pay-subtract-fee").check();
    await review(page);
    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    await expect(page.getByTestId("pay-result")).toContainText("215.74478273");
    await expect(page.getByTestId("pay-result-facts")).toContainText("0.00000192");
    // The payment's events reach the form behind the result: wait until the app is quiet again.
    await settledAfter(page, "draft_broadcast");
    const form = page.getByTestId("pay-address-form");
    await expect(form.locator(".field-error")).toHaveCount(0);
    await expect(page.getByTestId("pay-problem")).toHaveCount(0);
    await expect(form).not.toContainText("a normal payment can spend");

    await page.getByTestId("pay-result-close").click();
    await expect(page.getByTestId("pay-result")).toHaveCount(0);
    await expect(page.getByTestId("pay-address")).toHaveValue("");
    await expect(page.getByTestId("pay-amount")).toHaveValue("");
    await expect(form.locator(".field-error")).toHaveCount(0);
    await expect(page.getByTestId("pay-problem")).toHaveCount(0);
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
    // Nothing reaches the confirm sheet again: the payment was signed and sent once.
    await page.getByTestId("pay-address").press("Enter");
    await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_prepare" || c === "draft_broadcast"))).toEqual(["draft_prepare", "draft_broadcast"]);
  });
});

test.describe("Receive", () => {
  test("shows the address as QR code and text, and issues a new one", async ({ page }) => {
    const errors = await open(page);
    await page.getByTestId("sidebar-receive").click();
    await expect(page.getByTestId("receive-qr")).toBeVisible();
    const first = await page.getByTestId("receive-address").textContent();
    await expect(page.getByTestId("receive-qr")).toHaveAttribute("aria-label", `QR code of ${first}`);
    await page.getByTestId("receive-new-address").click();
    await expect(page.getByTestId("receive-address")).not.toHaveText(first!);
    expect(errors).toEqual([]);
  });

  test("creates, shows and removes payment requests", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-receive").click();
    await expect(page.getByTestId("requests-empty")).toHaveText("No payment requests yet.");
    await page.getByTestId("request-label").fill("Rent");
    await page.getByTestId("request-amount").fill("0.25");
    await page.getByTestId("request-create").click();
    await expect(page.getByTestId("request-sheet")).toBeVisible();
    await expect(page.getByTestId("request-uri")).toContainText("?amount=0.25&label=Rent");
    await expect(page.getByTestId("request-qr")).toBeVisible();
    await page.keyboard.press("Escape");
    // The form is cleared for the next request.
    await expect(page.getByTestId("request-label")).toHaveValue("");
    const row = page.getByTestId("requests-table").locator("tbody tr");
    await expect(row).toHaveCount(1);
    await expect(row).toContainText("Rent");
    await expect(row).toContainText("0.25000000");
    await row.getByRole("button", { name: "Show" }).click();
    await expect(page.getByTestId("request-sheet")).toBeVisible();
    await page.keyboard.press("Escape");
    // Focus goes back to the button that opened it, out of the sheet layer (U-03 D22).
    await expect(row.getByRole("button", { name: "Show" })).toBeFocused();
    await row.getByRole("button", { name: /Remove/ }).click();
    await expect(page.getByTestId("requests-empty")).toBeVisible();
  });

  test("a wallet switch shows and copies the other wallet's address (D24)", async ({ page }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) },
      });
    });
    await open(page, "two-wallets");
    const picker = page.getByTestId("toolbar-wallet-picker");
    const address = page.getByTestId("receive-address");
    // Wallet B's address, as Receive shows it fresh.
    await picker.selectOption({ label: "Savings" });
    await page.getByTestId("sidebar-receive").click();
    await expect(address).toHaveText(/^y/);
    const b = (await address.textContent())!;
    // Wallet A: a new address, then back to B.
    await picker.selectOption({ index: 0 });
    await expect(address).not.toHaveText(b);
    const a = (await address.textContent())!;
    await page.getByTestId("receive-new-address").click();
    await expect(address).not.toHaveText(a);
    await picker.selectOption({ label: "Savings" });
    await expect(address).toHaveText(b);
    await page.getByTestId("receive-copy-address").click();
    expect(await page.evaluate(() => (window as unknown as { __copied: string }).__copied)).toBe(b);
  });

  test("a request too long for a QR code says so", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("request-message").fill("x".repeat(240));
    await page.getByTestId("request-create").click();
    await expect(page.getByTestId("request-sheet")).toContainText("Resulting URI too long");
  });

  test("an invalid amount holds the request", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("request-amount").fill("abc");
    await expect(page.getByText("Enter an amount in DASH, for example 0.25.")).toBeVisible();
    await expect(page.getByTestId("request-create")).toBeDisabled();
    // Zero is not "no amount": it is refused, not dropped.
    await page.getByTestId("request-amount").fill("0");
    await expect(page.getByText("The amount must be larger than 0, or empty.")).toBeVisible();
    await expect(page.getByTestId("request-create")).toBeDisabled();
  });
});

test.describe("Activity", () => {
  test("table mode sorts by the engine and opens a row", async ({ page }) => {
    const errors = await open(page);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-mode-table").click();
    const table = page.getByTestId("tx-table");
    await expect(table).toBeVisible();
    await expect(page.getByTestId("tx-mode-table")).toHaveAttribute("aria-pressed", "true");
    await expect(table.locator("th[aria-sort]").first()).toHaveAttribute("aria-sort", "descending");
    await page.getByTestId("tx-sort-amount").click();
    await expect(table.locator("th[aria-sort]")).toHaveAttribute("aria-sort", "descending");
    await expect(table.locator("tbody tr").first()).toContainText("+3.76941262");
    await page.getByTestId("tx-sort-amount").click();
    await expect(table.locator("tbody tr").first()).toContainText("-");
    await table.locator(".table-open").first().click();
    await expect(page.getByTestId("tx-detail-facts")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("filters by date and minimum amount", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-activity").click();
    // 45 Core records, the fixture's 2 Platform-address rows and 3 Shielded ones.
    await expect(page.getByTestId("tx-footer")).toHaveText("50 transactions");
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-footer")).toHaveText("2 transactions");
    await page.getByTestId("tx-date").selectOption("all");
    await page.getByTestId("tx-min-amount").fill("3");
    await expect(page.getByTestId("tx-footer")).not.toHaveText("50 transactions");
    const n = Number((await page.getByTestId("tx-footer").textContent())!.split(" ")[0]);
    expect(n).toBeGreaterThan(0);
    expect(n).toBeLessThan(50);
    await page.getByTestId("tx-min-amount").fill("x");
    await expect(page.getByTestId("tx-min-amount")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByTestId("tx-min-amount-error")).toBeVisible();
    await expect(page.getByTestId("tx-footer")).toHaveText("50 transactions");
  });

  test("Today moves with the local day once the window comes back (D25)", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-footer")).toHaveText("2 transactions");
    // The renderer's clock moves on to the next day while the window is away; nothing is dated then.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.now = new Date(f.now.getFullYear(), f.now.getMonth(), f.now.getDate() + 1, 12);
      window.dispatchEvent(new Event("focus"));
    });
    await expect(page.getByTestId("tx-footer")).toHaveText("0 transactions");
  });

  test("labels a transaction and abandons an unconfirmed send", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await sendWithPassphrase(page);
    await page.getByTestId("pay-result-show").click();
    await expect(page.getByTestId("tx-detail-facts")).toBeVisible();

    await page.getByTestId("tx-label-input").fill("Lunch");
    await page.getByTestId("tx-label-save").click();
    await expect(page.getByTestId("tx-detail").getByText("Lunch").first()).toBeVisible();

    await page.getByTestId("tx-resend").click();
    await expect(page.getByTestId("tx-action-note")).toBeVisible();
    await page.getByTestId("tx-abandon").click();
    await page.getByTestId("tx-abandon-confirm").click();
    await expect(page.getByTestId("tx-detail")).toContainText("Abandoned");
    await expect(page.getByTestId("tx-abandon")).toHaveCount(0);
    await expect(page.getByTestId("tx-action-note")).toHaveText("The transaction was abandoned.");
  });
});

// DP3-02 / DP3-03 on U-03's screens: the fixture's `dashpay-registered` wallet ("alice") has a
// payment from Bob ("Bob Builder") and one to Dave (no display name) attributed.
test.describe("DashPay attribution and username", () => {
  const attributed = (page: Page) =>
    world(page, (w) => {
      const of = (id: string) => w.wallets[0].rows.find((r) => r.counterparty?.identity === id)!.txid.slice(0, 10);
      return { bob: of("8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN"), dave: of("DpPxRMjXzAv1hJDCFnRWGDSEZtqVvtxRrD9AAbGvVdsY") };
    });

  test("Activity names the contact of a payment, in rows, the table and the detail sheet", async ({ page }) => {
    const errors = await open(page, "dashpay-registered");
    const { bob, dave } = await attributed(page);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId(`tx-row-${bob}-0`).locator(".tx-row-title")).toHaveText("Bob Builder");
    await expect(page.getByTestId(`tx-row-${dave}-0`).locator(".tx-row-title")).toHaveText("@dave");
    await page.getByTestId(`tx-row-${bob}-0`).click();
    const contact = page.getByTestId("tx-detail-contact");
    await expect(contact.locator(".detail-label")).toHaveText("Received from contact");
    await expect(contact.locator(".tx-detail-contact-name")).toHaveText("Bob Builder");
    await expect(page.getByTestId("tx-detail-summary")).toContainText("Bob Builder");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await page.getByTestId("tx-mode-table").click();
    await expect(page.getByTestId("tx-table").locator("tbody tr", { hasText: "@dave" })).toHaveCount(1);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("a wallet without DashPay names no contact", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-group-0")).toBeVisible();
    await expect(page.getByText("Bob Builder")).toHaveCount(0);
  });

  test("Receive's link and requests carry the main name as user= (DP3-03)", async ({ page }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) },
      });
    });
    const errors = await open(page, "dashpay-registered");
    await page.getByTestId("sidebar-receive").click();
    const address = (await page.getByTestId("receive-address").textContent())!;
    await expect(page.getByTestId("receive-copy-uri")).toBeEnabled();
    await page.getByTestId("receive-copy-uri").click();
    expect(await page.evaluate(() => (window as unknown as { __copied: string }).__copied)).toBe(`dash:${address}?user=alice`);
    await page.getByTestId("request-label").fill("Rent");
    await page.getByTestId("request-create").click();
    await expect(page.getByTestId("request-uri")).toContainText("?label=Rent&user=alice");
    await expect(page.getByTestId("request-username")).toHaveText(/@alice/);
    expect(errors).toEqual([]);
  });
});

test.describe("axe", () => {
  for (const theme of THEMES)
    for (const view of WALLET_VIEWS)
      test(`axe ${view} ${theme}`, async ({ page }) => {
        const errors = trackPageErrors(page);
        await showWalletView(page, view, theme);
        expect(await seriousAxeViolations(page)).toEqual([]);
        expect(errors).toEqual([]);
      });
});
