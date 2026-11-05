import { expect, test, type Page } from "@playwright/test";
import type { World } from "../src/fixture/scenarios";
import { ENTRY_TEXT } from "../src/fiat/entryText";
import { full } from "../src/format/amount";
import { fiatText, minorToDuffs } from "../src/format/fiat";
import { fixtureUrl, trackPageErrors } from "./helpers";

// MP-01 slice (c) on the fixture backend: the fiat toggle of Pay's amount field (to an address and to
// a contact) and of Receive's request amount, and the values beside the amounts on Confirm. Fiat is
// converted to duffs in the renderer, half to even; the engine sees only the DASH text it parses.

const PRICE = "49.0967";
/** A Mainnet P2PKH address the fixture accepts. */
const MAIN_PAYEE = "XXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
const duffsOf = (minor: bigint) => Number(minorToDuffs(minor, PRICE, "USD"));
const dash = (duffs: number) => `${full(duffs, false, "DASH")} DASH`;

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function payToAddress(page: Page) {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  await page.getByTestId("pay-address").fill(MAIN_PAYEE);
}

/** The price over 30 minutes old, announced as the engine's `rates` event. */
async function ageThePrice(page: Page) {
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.rates.cached = f.world.rates.cached.map((r) => ({ ...r, rateAt: r.rateAt - 3600 }));
    f.world.rates.serves = null;
    f.emit({ kind: "rates" });
  });
}

test("10.00 USD fills the DASH line, and Confirm shows those duffs as the amount, its value beside", async ({ page }) => {
  const errors = await open(page, "fiat");
  await payToAddress(page);
  const duffs = duffsOf(1000n);
  await page.getByTestId("pay-amount-toggle").click();
  await expect(page.getByTestId("pay-amount-toggle")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("Amount (USD)")).toBeVisible();
  await page.getByTestId("pay-amount").fill("10.00");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(duffs));
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).waitFor();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-amount")).toContainText(full(duffs, false, "DASH"));
  await expect(page.getByTestId("pay-confirm-amount-fiat")).toHaveText(fiatText(duffs, PRICE, "USD", "en-US")!);
  await expect(page.getByTestId("pay-confirm-fiat")).toHaveText(/^\$\d+\.\d\d$/);
  await expect(page.getByTestId("pay-confirm-fee-fiat")).toBeVisible();
  expect(errors).toEqual([]);
});

test("in DASH the field shows its value; switching alone never changes the amount", async ({ page }) => {
  await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount").fill("1.23456789");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(fiatText(123_456_789, PRICE, "USD", "en-US")!);
  await page.getByTestId("pay-amount-toggle").click();
  await expect(page.getByTestId("pay-amount")).toHaveValue("60.61");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(123_456_789));
  await page.getByTestId("pay-amount-toggle").click();
  await expect(page.getByTestId("pay-amount")).toHaveValue("1.23456789");
});

test("fiat that is not an amount is refused, and nothing is estimated", async ({ page }) => {
  await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-amount").fill("10.001");
  await expect(page.getByText(ENTRY_TEXT.invalid("USD"))).toBeVisible();
  await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
});

test("a stale price disables the toggle and says why", async ({ page }) => {
  await open(page, "fiat-stale");
  await payToAddress(page);
  await expect(page.getByTestId("pay-amount-toggle")).toBeDisabled();
  await expect(page.getByTestId("pay-amount-other-why")).toHaveText(ENTRY_TEXT.notFresh);
  await page.getByTestId("pay-amount").fill("1");
  await expect(page.getByTestId("pay-amount-other")).toHaveClass(/is-stale/);
});

test("losing the fresh price turns the field back to DASH with the same duffs", async ({ page }) => {
  await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-amount").fill("10.00");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(duffsOf(1000n)));
  await ageThePrice(page);
  await expect(page.getByTestId("pay-amount-other-note")).toHaveText(ENTRY_TEXT.backToDash);
  await expect(page.getByText("Amount (DASH)")).toBeVisible();
  await expect(page.getByTestId("pay-amount")).toHaveValue(full(duffsOf(1000n), false, "DASH"));
  await expect(page.getByTestId("pay-amount-toggle")).toBeDisabled();
});

test("Max fills DASH, as always", async ({ page }) => {
  await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-max").click();
  await expect(page.getByTestId("pay-amount-toggle")).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByText("Amount (DASH)")).toBeVisible();
  await expect(page.getByTestId("pay-amount")).toHaveValue(/^\d+\.\d{8}$/);
});

for (const scenario of ["funded", "fiat-off"]) {
  test(`${scenario}: no toggle and no value line`, async ({ page }) => {
    await open(page, scenario);
    await payToAddress(page);
    await page.getByTestId("pay-amount").fill("1");
    await expect(page.getByTestId("pay-amount-toggle")).toHaveCount(0);
    await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
  });
}

test("discreet mode: no toggle and no value line", async ({ page }) => {
  await open(page, "fiat");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
  await payToAddress(page);
  await page.getByTestId("pay-amount").fill("1");
  await expect(page.getByTestId("pay-amount-toggle")).toHaveCount(0);
  await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
});

// The contact form shares AmountFields with Pay to an address. The fixture's contacts pay testnet
// addresses, which a Mainnet session refuses, so this stops at the converted amount; Confirm is the
// first test's.
test("paying a contact takes the amount in fiat too", async ({ page }) => {
  await open(page, "fiat-contacts");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-frequent-chip").first().click();
  await page.getByTestId("pay-amount-toggle").click();
  await expect(page.getByText("Amount (USD)")).toBeVisible();
  await page.getByTestId("pay-amount").fill("5");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(duffsOf(500n)));
  await page.getByTestId("pay-amount-toggle").click();
  await expect(page.getByTestId("pay-amount")).toHaveValue(full(duffsOf(500n), false, "DASH"));
});

test.describe("Receive", () => {
  test("a request amount typed in fiat stores the duffs it converts to", async ({ page }) => {
    await open(page, "fiat");
    await page.getByTestId("sidebar-receive").click();
    const duffs = duffsOf(2550n);
    await page.getByTestId("request-amount-toggle").click();
    await page.getByTestId("request-amount").fill("25.50");
    await expect(page.getByTestId("request-amount-other")).toHaveText(dash(duffs));
    await page.getByTestId("request-create").and(page.locator(":enabled")).waitFor();
    await page.getByTestId("request-create").click();
    await expect.poll(() => world(page, (w) => w.wallets[0].receive.requests[0]?.amount)).toBe(duffs);
  });

  test("fiat that is not an amount never makes an any-amount request", async ({ page }) => {
    await open(page, "fiat");
    await page.getByTestId("sidebar-receive").click();
    const before = await world(page, (w) => w.wallets[0].receive.requests.length);
    await page.getByTestId("request-amount-toggle").click();
    await page.getByTestId("request-amount").fill("25.505");
    await expect(page.getByText(ENTRY_TEXT.invalid("USD"))).toBeVisible();
    await expect(page.getByTestId("request-create")).toBeDisabled();
    await page.getByTestId("request-amount").press("Enter");
    expect(await world(page, (w) => w.wallets[0].receive.requests.length)).toBe(before);
  });
});
