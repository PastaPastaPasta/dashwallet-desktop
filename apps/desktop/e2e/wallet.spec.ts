import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
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
    await expect(page.getByText("below the dust threshold")).toBeVisible();
    await expect(page.getByTestId("pay-continue")).toBeDisabled();

    await page.getByTestId("pay-amount").fill("500");
    await expect(page.getByText("The amount exceeds your balance.")).toBeVisible();

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

    // A wrong passphrase is refused and cleared; nothing is signed.
    await sendWithPassphrase(page, "wrong");
    await expect(page.getByTestId("pay-confirm-error")).toHaveText("The passphrase you entered was incorrect.");
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveValue("");
    expect(await world(page, (w) => Object.keys(w.send.prepared).length)).toBe(0);

    await sendWithPassphrase(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    const txid = (await world(page, (w) => w.wallets[0].rows[0].txid)) as string;
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

  test("with the payment setting off, the unlocked vault signs and Cancel has the focus", async ({ page }) => {
    await open(page);
    await world(page, (w) => (w.paymentAuth = false));
    await fillPayment(page, PAYEE, "0.5");
    await review(page);
    await expect(page.getByTestId("pay-confirm-cancel")).toBeFocused();
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
    // Return on Cancel does not send.
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
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
    await world(page, (w) => (w.send.broadcast = "ok"));
    await page.getByTestId("pay-result-retry").click();
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    expect(await world(page, (w) => w.wallets[0].rows.filter((r) => r.amount === -50_000_226).length)).toBe(2);
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
    expect(await world(page, (w) => w.wallets[0].coins.some((u) => u.reserved))).toBe(false);
  });

  test("coin control: selected coins, mixed funds, a custom fee rate, a change address", async ({ page }) => {
    await open(page);
    await fillPayment(page, PAYEE, "0.5");
    await page.getByTestId("pay-advanced").locator("summary").click();

    await page.getByTestId("coin-choice-mixed").check();
    await expect(page.getByTestId("pay-amount")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByText(/^Not enough fully mixed funds: 0\.00.* available\.$/)).toBeVisible();

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
    await row.getByRole("button", { name: /Remove/ }).click();
    await expect(page.getByTestId("requests-empty")).toBeVisible();
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
    await expect(page.getByTestId("tx-footer")).toHaveText("45 transactions");
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-footer")).toHaveText("2 transactions");
    await page.getByTestId("tx-date").selectOption("all");
    await page.getByTestId("tx-min-amount").fill("3");
    await expect(page.getByTestId("tx-footer")).not.toHaveText("45 transactions");
    const n = Number((await page.getByTestId("tx-footer").textContent())!.split(" ")[0]);
    expect(n).toBeGreaterThan(0);
    expect(n).toBeLessThan(45);
    await page.getByTestId("tx-min-amount").fill("x");
    await expect(page.getByTestId("tx-min-amount")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByTestId("tx-footer")).toHaveText("45 transactions");
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
  });
});

test.describe("axe", () => {
  for (const theme of THEMES)
    for (const view of WALLET_VIEWS)
      test(`axe ${view} ${theme}`, async ({ page }) => {
        const errors = trackPageErrors(page);
        await showWalletView(page, view, theme);
        await fontsReady(page);
        expect(await seriousAxeViolations(page)).toEqual([]);
        expect(errors).toEqual([]);
      });
});
