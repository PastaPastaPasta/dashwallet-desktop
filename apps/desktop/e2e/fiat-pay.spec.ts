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

/** A new fresh USD price, announced as the engine's `rates` event. */
async function newPrice(page: Page, price: string) {
  await page.evaluate((price) => {
    const f = window.__dwFixture!;
    f.world.rates.cached = f.world.rates.cached.map((r) => (r.code === "USD" ? { ...r, price } : r));
    f.emit({ kind: "rates" });
  }, price);
}

/** Prices turned on or off through the engine's command, as Settings does, while a form is open. */
async function setPrices(page: Page, enabled: boolean) {
  await page.evaluate(
    (enabled) =>
      (window as unknown as { __TAURI_INTERNALS__: { invoke(c: string, a: unknown): Promise<unknown> } }).__TAURI_INTERNALS__.invoke("set_rates_settings", {
        settings: { enabled, currency: "USD", disclosed: true },
      }),
    enabled,
  );
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

test("a new fresh price converts the typed fiat again; an open Confirm keeps its amount", async ({ page }) => {
  const errors = await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-amount").fill("10.00");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(duffsOf(1000n)));
  const at = (price: string) => Number(minorToDuffs(1000n, price, "USD"));
  await newPrice(page, "51.2345");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(at("51.2345")));
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).waitFor();
  await page.getByTestId("pay-continue").click();
  const amount = full(at("51.2345"), false, "DASH");
  await expect(page.getByTestId("pay-confirm-amount")).toContainText(amount);
  await expect(page.getByTestId("pay-confirm-amount-fiat")).toHaveText(fiatText(at("51.2345"), "51.2345", "USD", "en-US")!);
  // Another price while Confirm is open: the form behind converts again and re-estimates its new
  // amount, and only the values beside Confirm's amounts move. What is sent is Confirm's amount.
  await newPrice(page, "60.0000");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(at("60.0000")));
  await expect(page.getByTestId("pay-confirm-amount-fiat")).toHaveText(fiatText(at("51.2345"), "60.0000", "USD", "en-US")!);
  await expect
    .poll(() =>
      page.evaluate(
        (duffs) => Object.values(window.__dwFixture!.world.send.drafts).some((d) => d.recipients[0]?.amount === duffs),
        at("60.0000"),
      ),
    )
    .toBe(true);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText(amount);
  await expect(page.getByTestId("pay-confirm-amount")).not.toContainText(full(at("60.0000"), false, "DASH"));
  const before = await world(page, (w) => w.wallets[0].rows.length);
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect.poll(() => world(page, (w) => w.wallets[0].rows.length)).toBe(before + 1);
  const sent = await world(page, (w) => w.wallets[0].rows[0]);
  expect(-sent.amount - (sent.fee ?? 0)).toBe(at("51.2345"));
  expect(errors).toEqual([]);
});

test("a new price that makes the typed fiat too large clears the DASH amount and keeps Review closed", async ({ page }) => {
  const errors = await open(page, "fiat");
  await payToAddress(page);
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-amount").fill("4000000000");
  await expect(page.getByTestId("pay-amount-other")).toHaveText(dash(duffsOf(400_000_000_000n)));
  // Over 81M DASH: the estimate refuses it, for a reason about the DASH amount.
  await expect(page.getByTestId("pay-problem")).toBeVisible();
  await newPrice(page, "40.0000");
  await expect(page.getByText(ENTRY_TEXT.tooLarge)).toBeVisible();
  await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
  await expect(page.getByTestId("pay-problem")).toHaveCount(0);
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  expect(errors).toEqual([]);
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

// C5b: fiat whose duffs are past what a number holds exactly (over 90 million DASH) is refused with
// a reason, never left as an empty DASH amount; yen has no minor unit.
const TOO_LARGE = [
  ["fiat", "5000000000"],
  ["fiat", "999999999999999.99"],
  ["fiat-jpy", "1000000000000"],
] as const;
for (const [scenario, text] of TOO_LARGE) {
  test(`C5b ${scenario} ${text}: Pay refuses it, with the reason`, async ({ page }) => {
    await open(page, scenario);
    await payToAddress(page);
    await page.getByTestId("pay-amount-toggle").click();
    await page.getByTestId("pay-amount").fill(text);
    await expect(page.getByText(ENTRY_TEXT.tooLarge)).toBeVisible();
    await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
  });

  test(`C5b ${scenario} ${text}: Receive refuses it and makes no request`, async ({ page }) => {
    await open(page, scenario);
    await page.getByTestId("sidebar-receive").click();
    const before = await world(page, (w) => w.wallets[0].receive.requests.length);
    await page.getByTestId("request-amount-toggle").click();
    await page.getByTestId("request-amount").fill(text);
    await expect(page.getByText(ENTRY_TEXT.tooLarge)).toBeVisible();
    await expect(page.getByTestId("request-create")).toBeDisabled();
    await page.getByTestId("request-amount").press("Enter");
    await page.waitForTimeout(500);
    expect(await world(page, (w) => w.wallets[0].receive.requests.length)).toBe(before);
  });
}

test("C5b: paying a contact refuses a fiat amount too large to convert", async ({ page }) => {
  await open(page, "fiat-contacts");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-frequent-chip").first().click();
  await page.getByTestId("pay-amount-toggle").click();
  await page.getByTestId("pay-amount").fill("5000000000");
  await expect(page.getByText(ENTRY_TEXT.tooLarge)).toBeVisible();
  await expect(page.getByTestId("pay-amount-other")).toHaveCount(0);
});

// Off clears the cached price (rates-engine §1): the toggle and the value line go, a field in fiat is
// DASH again with the same duffs, and on again nothing converts until a new fetch.
for (const form of [
  { name: "Pay", go: payToAddress, id: "pay-amount" },
  { name: "Receive", go: (page: Page) => page.getByTestId("sidebar-receive").click(), id: "request-amount" },
]) {
  test(`${form.name}: prices off while typing in fiat turn the field back to DASH with the same duffs`, async ({ page }) => {
    const errors = await open(page, "fiat");
    await form.go(page);
    const duffs = full(duffsOf(1234n), false, "DASH");
    await page.getByTestId(`${form.id}-toggle`).click();
    await page.getByTestId(form.id).fill("12.34");
    await expect(page.getByTestId(`${form.id}-other`)).toHaveText(`${duffs} DASH`);

    await setPrices(page, false);
    await expect(page.getByTestId(`${form.id}-toggle`)).toHaveCount(0);
    await expect(page.getByTestId(`${form.id}-other`)).toHaveCount(0);
    await expect(page.getByTestId(form.id)).toHaveValue(duffs);
    await expect(page.getByTestId(`${form.id}-other-note`)).toHaveText(ENTRY_TEXT.backToDash);

    await setPrices(page, true);
    await expect(page.getByTestId(`${form.id}-toggle`)).toBeDisabled();
    await expect(page.getByTestId(`${form.id}-other-why`)).toHaveText(ENTRY_TEXT.unavailable);
    await expect(page.getByTestId(`${form.id}-other`)).toHaveCount(0);
    await expect(page.getByTestId(form.id)).toHaveValue(duffs);
    expect(errors).toEqual([]);
  });
}

// Fiat that converts to nothing leaves no DASH amount: going back to DASH says so, never "kept".
for (const form of [
  { name: "Pay", go: payToAddress, id: "pay-amount" },
  { name: "Receive", go: (page: Page) => page.getByTestId("sidebar-receive").click(), id: "request-amount" },
]) {
  for (const how of ["the toggle", "prices off"] as const) {
    test(`${form.name}: leaving fiat that is not an amount by ${how} says no amount is set`, async ({ page }) => {
      const errors = await open(page, "fiat");
      await form.go(page);
      await page.getByTestId(`${form.id}-toggle`).click();
      await page.getByTestId(form.id).fill("12.345");
      await expect(page.getByText(ENTRY_TEXT.invalid("USD"))).toBeVisible();
      if (how === "the toggle") await page.getByTestId(`${form.id}-toggle`).click();
      else await setPrices(page, false);
      await expect(page.getByTestId(`${form.id}-other-note`)).toHaveText(ENTRY_TEXT.notKept("USD"));
      await expect(page.getByTestId(form.id)).toHaveValue("");
      await expect(page.getByText("Amount (DASH)")).toBeVisible();
      expect(errors).toEqual([]);
    });
  }
}

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

  test("each stored request shows its value now; discreet hides it, and prices off remove it", async ({ page }) => {
    const errors = await open(page, "fiat");
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("request-amount").fill("1.5");
    await page.getByTestId("request-create").and(page.locator(":enabled")).waitFor();
    await page.getByTestId("request-create").click();
    await page.keyboard.press("Escape");
    const id = await world(page, (w) => w.wallets[0].receive.requests[0].id);
    const line = page.getByTestId(`request-fiat-${id}`);
    await expect(line).toHaveText(fiatText(150_000_000, PRICE, "USD", "en-US")!);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await expect(line).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await expect(line).toBeVisible();
    await setPrices(page, false);
    await expect(line).toHaveCount(0);
    expect(errors).toEqual([]);
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
