import { expect, test, type Page } from "@playwright/test";
import type { World } from "../src/fixture/scenarios";
import { fiatText } from "../src/format/fiat";
import { homeBalance } from "../src/format/homeBalance";
import { shieldedDuffs } from "../src/api/queries";
import type { ShieldedStatus } from "../src/api/generated/ShieldedStatus";
import { MAIN_PHRASE } from "../src/fixture/vault";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import type { TxRow } from "../src/api/generated/TxRow";

// MP-01 on the fixture backend (src/fixture/rates.ts): the Home hero and breakdown in fiat, the
// stale, volatile and unavailable lines, discreet mode, the one-time disclosure (Home card,
// onboarding's last step), Settings ▸ General ▸ Local currency, and Activity with the transaction
// detail (the value now, and the price stamp a payment was first seen with). Prices are Mainnet's;
// the testnet scenarios show none.

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.locator("[data-testid=onboarding], [data-testid=hero]").first().waitFor();
  return errors;
}

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;
const navigate = (page: Page, route: "home" | "settings" | "activity") => page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);

/** The hero's total and each pool in duffs, as Home adds them up from the same commands. */
async function totals(page: Page) {
  const [core, shielded, platform] = await page.evaluate(async () => {
    const w = window.__dwFixture!.world;
    const walletId = w.wallets[0].summary.walletId;
    const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke(c: string, a: unknown): Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
    return [w.wallets[0].summary.balances!, await invoke("shielded_status", { walletId }), await invoke("platform_addresses", { walletId })] as const;
  });
  return homeBalance(core, shieldedDuffs(shielded as ShieldedStatus | null), (platform as { duffs: number | null } | null)?.duffs ?? null);
}

test("fresh USD: the hero and each breakdown row show their value, from the exact integer", async ({ page }) => {
  const errors = await open(page, "fiat");
  const home = await totals(page);
  await expect(page.getByTestId("hero-fiat")).toHaveText(fiatText(home.total, "49.0967", "USD", "en-US")!);
  await expect(page.getByTestId("breakdown-core-fiat")).toHaveText(fiatText(home.pools.core!, "49.0967", "USD", "en-US")!);
  await expect(page.getByTestId("hero-fiat")).not.toHaveClass(/is-stale/);
  await expect(page.getByTestId("hero-rates-note")).toHaveCount(0);
  // The band grows by the fiat line only.
  const band = await page.getByTestId("hero-band").boundingBox();
  expect(Math.round(band!.height)).toBe(236 + 18);
  await expect(page.getByTestId("rates-consent-card")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("testnet shows no fiat and keeps the band's height", async ({ page }) => {
  await open(page, "funded");
  await expect(page.getByTestId("hero-fiat-row")).toHaveCount(0);
  await expect(page.locator(".fiat-line")).toHaveCount(0);
  await expect(page.getByTestId("rates-consent-card")).toHaveCount(0);
  const band = await page.getByTestId("hero-band").boundingBox();
  expect(Math.round(band!.height)).toBe(236);
  expect(await world(page, (w) => w.rates.requests)).toBe(0);
});

test("discreet mode hides every fiat line with the amounts", async ({ page }) => {
  await open(page, "fiat");
  await expect(page.getByTestId("hero-fiat")).toBeVisible();
  await page.getByTestId("hero-amount").click();
  await expect(page.getByTestId("hero-fiat")).toHaveCount(0);
  await expect(page.locator(".fiat-line")).toHaveCount(0);
  await page.getByTestId("hero-amount").click();
  await expect(page.getByTestId("hero-fiat")).toBeVisible();
});

test("a stale price shows with the caution line and its time", async ({ page }) => {
  await open(page, "fiat-stale");
  await expect(page.getByTestId("hero-fiat")).toHaveClass(/is-stale/);
  await expect(page.getByTestId("hero-fiat")).toHaveAttribute("title", /^Price from .+, may be out of date$/);
  await expect(page.getByTestId("hero-rates-note")).toHaveText("Prices are over 30 minutes old.");
});

test("a volatile price says so", async ({ page }) => {
  await open(page, "fiat-volatile");
  await expect(page.getByTestId("hero-fiat")).toBeVisible();
  await expect(page.getByTestId("hero-rates-note")).toHaveText("Prices moved more than 50% since the last update.");
});

test("unavailable prices show no number anywhere, only why", async ({ page }) => {
  await open(page, "fiat-unavailable");
  await expect(page.getByTestId("hero-rates-note")).toHaveText("Prices unavailable");
  await expect(page.getByTestId("hero-rates-note")).toHaveAttribute("title", /could not be fetched/);
  await expect(page.locator(".fiat-line")).toHaveCount(0);
  await expect(page.getByTestId("hero")).not.toContainText("$");
});

test("prices off: no fiat, no line, no attempt", async ({ page }) => {
  await open(page, "fiat-off");
  await expect(page.getByTestId("hero-fiat-row")).toHaveCount(0);
  await expect(page.locator(".fiat-line")).toHaveCount(0);
  expect(await world(page, (w) => w.rates.requests)).toBe(0);
});

test("yen has no minor unit", async ({ page }) => {
  await open(page, "fiat-jpy");
  const home = await totals(page);
  await expect(page.getByTestId("hero-fiat")).toHaveText(fiatText(home.total, "7873.9517", "JPY", "en-US")!);
  await expect(page.getByTestId("hero-fiat")).toHaveText(/^¥[\d,]+$/);
});

test("a 0.1 install on Mainnet: Home asks once, and OK turns prices on in the locale's currency", async ({ page }) => {
  await open(page, "fiat-disclosure");
  const card = page.getByTestId("rates-consent-card");
  await expect(card).toContainText("Show prices in US Dollar?");
  await expect(card).toContainText("The wallet fetches exchange rates from CTX");
  await expect(page.locator(".fiat-line")).toHaveCount(0);
  await page.getByTestId("rates-consent-ok").click();
  await expect(card).toHaveCount(0);
  expect(await world(page, (w) => w.rates.settings)).toEqual({ enabled: true, currency: "USD", disclosed: true });
});

test("a 0.1 install on Mainnet: Turn off answers once and fetches nothing", async ({ page }) => {
  await open(page, "fiat-disclosure");
  await page.getByTestId("rates-consent-off").click();
  await expect(page.getByTestId("rates-consent-card")).toHaveCount(0);
  expect(await world(page, (w) => w.rates.settings)).toMatchObject({ enabled: false, disclosed: true });
  expect(await world(page, (w) => w.rates.requests)).toBe(0);
});

test("a new wallet on Mainnet sees the disclosure on onboarding's last step", async ({ page }) => {
  await open(page, "fiat-first-run");
  // Restoring is the shortest path to the last step.
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(MAIN_PHRASE);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("onboarding-restoreOptions").waitFor();
  await page.getByTestId("restore-finish").click();
  await page.getByTestId("onboarding-choosePassphrase").waitFor();
  await page.getByTestId("onboarding-passphrase-new").fill("restored pass");
  await page.getByTestId("onboarding-passphrase-repeat").fill("restored pass");
  await page.getByTestId("onboarding-passphrase-submit").click();
  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  await expect(page.getByTestId("onboarding-rates")).toContainText("rates.ctx.com");
  await expect(page.getByTestId("onboarding-rates-on")).toBeChecked();
  expect(await world(page, (w) => w.rates.settings.disclosed)).toBe(false);
  await page.getByTestId("onboarding-open").click();
  await expect(page.getByTestId("hero")).toBeVisible();
  expect(await world(page, (w) => w.rates.settings)).toEqual({ enabled: true, currency: "USD", disclosed: true });
  await expect(page.getByTestId("rates-consent-card")).toHaveCount(0);
});

test.describe("Settings ▸ General ▸ Local currency", () => {
  async function openSettings(page: Page, scenario: string, theme: Theme = "light") {
    const errors = await open(page, scenario, theme);
    await navigate(page, "settings");
    await page.getByTestId("settings-currency").waitFor();
    return errors;
  }

  test("search and pick a currency: Home follows", async ({ page }) => {
    await openSettings(page, "fiat");
    await expect(page.getByTestId("settings-currency-on")).toBeChecked();
    await expect(page.getByTestId("settings-currency-status")).toHaveText(/^Updated .+ from rates\.ctx\.com\.$/);
    await expect(page.getByTestId("settings-currency-USD")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("settings-currency-search").fill("yen");
    await expect(page.locator(".currency-row")).toHaveCount(1);
    await page.getByTestId("settings-currency-JPY").click();
    await expect.poll(() => world(page, (w) => w.rates.settings.currency)).toBe("JPY");
    await page.getByTestId("settings-currency-search").fill("zzz");
    await expect(page.getByTestId("settings-currency-picker")).toContainText("No currency matches the search.");
    await navigate(page, "home");
    await expect(page.getByTestId("hero-fiat")).toHaveText(/^¥/);
  });

  test("the switch off hides every fiat line and stops attempts", async ({ page }) => {
    await openSettings(page, "fiat");
    await page.getByTestId("settings-currency-on").uncheck();
    await expect.poll(() => world(page, (w) => w.rates.settings.enabled)).toBe(false);
    await expect(page.getByTestId("settings-currency-picker")).toHaveCount(0);
    await expect(page.getByTestId("settings-currency-refresh")).toHaveCount(0);
    await navigate(page, "home");
    await expect(page.getByTestId("hero")).toBeVisible();
    await expect(page.locator(".fiat-line")).toHaveCount(0);
    expect(await world(page, (w) => w.rates.requests)).toBe(0);
  });

  test("Update now makes one attempt, then waits 30 s", async ({ page }) => {
    await openSettings(page, "fiat");
    await page.getByTestId("settings-currency-refresh").click();
    await expect.poll(() => world(page, (w) => w.rates.requests)).toBe(1);
    await page.getByTestId("settings-currency-refresh").click();
    await expect(page.getByTestId("settings-currency-problem")).toHaveText("Prices were just updated. Try again in a moment.");
    expect(await world(page, (w) => w.rates.requests)).toBe(1);
  });

  test("on testnet the switch says test coins have no price", async ({ page }) => {
    await openSettings(page, "funded");
    await page.getByTestId("settings-currency-on").check();
    await expect.poll(() => world(page, (w) => w.rates.settings)).toEqual({ enabled: true, currency: "USD", disclosed: true });
    await expect(page.getByTestId("settings-currency-status")).toContainText("Test coins have no price");
    await expect(page.getByTestId("settings-currency-refresh")).toHaveCount(0);
    expect(await world(page, (w) => w.rates.requests)).toBe(0);
  });

  for (const theme of THEMES) {
    test(`0 serious axe issues, ${theme}`, async ({ page }) => {
      await openSettings(page, "fiat", theme);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      await navigate(page, "home");
      await page.getByTestId("hero-fiat").waitFor();
      expect(await seriousAxeViolations(page)).toEqual([]);
    });
  }
});

test.describe("Activity and the transaction detail", () => {
  const rowId = (r: Pick<TxRow, "txid" | "recordIndex">) => `tx-row-${r.txid.slice(0, 10)}-${r.recordIndex}`;
  const rows = (page: Page) => world(page, (w) => w.wallets[0].rows.slice(0, 5));

  async function openActivity(page: Page, scenario: string) {
    const errors = await open(page, scenario);
    await navigate(page, "activity");
    await page.getByTestId("tx-list").waitFor();
    return errors;
  }

  test("each row shows its value now, from the exact integer, and discreet mode hides it", async ({ page }) => {
    const errors = await openActivity(page, "fiat");
    const [first, , sent] = await rows(page);
    await expect(page.getByTestId(`${rowId(first)}-fiat`)).toHaveText(fiatText(first.amount, "49.0967", "USD", "en-US")!);
    await expect(page.getByTestId(`${rowId(sent)}-fiat`)).toHaveText(fiatText(sent.amount, "49.0967", "USD", "en-US")!);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await expect(page.locator(".tx-row-fiat")).toHaveCount(0);
    await expect(page.getByTestId(rowId(first))).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("the table has a value column only while a price shows", async ({ page }) => {
    await openActivity(page, "fiat-stale");
    await page.getByTestId("tx-mode-table").click();
    await expect(page.getByTestId("tx-table-value")).toHaveText("Value (USD)");
    const [first] = await rows(page);
    const cell = page.getByTestId(`table-${rowId(first)}-fiat`);
    await expect(cell).toHaveText(fiatText(first.amount, "49.0967", "USD", "en-US")!);
    await expect(cell).toHaveClass(/is-stale/);
    await expect(cell).toHaveAttribute("title", /^Price from .+, may be out of date$/);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await expect(page.getByTestId("tx-table-value")).toHaveCount(0);
  });

  for (const scenario of ["funded", "fiat-off", "fiat-unavailable"]) {
    test(`${scenario}: no value column, no row value`, async ({ page }) => {
      await openActivity(page, scenario);
      await expect(page.locator(".tx-row-fiat")).toHaveCount(0);
      await page.getByTestId("tx-mode-table").click();
      await expect(page.getByTestId("tx-table")).toBeVisible();
      await expect(page.getByTestId("tx-table-value")).toHaveCount(0);
    });
  }

  test("the detail says what a payment was worth when first seen, in that stamp's currency, and its value now", async ({ page }) => {
    await openActivity(page, "fiat");
    const [received, unstamped, sent] = await rows(page);

    await page.getByTestId(rowId(received)).click();
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveText(`${fiatText(received.amount, "48.1203", "USD", "en-US")} when received`);
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveAttribute("title", /from rates\.ctx\.com$/);
    await expect(page.getByTestId("tx-detail-value-now-text")).toHaveText(fiatText(received.amount, "49.0967", "USD", "en-US")!);
    await page.keyboard.press("Escape");

    // A payment seen before prices were on keeps none: no back-filled price.
    await page.getByTestId(rowId(unstamped)).click();
    await expect(page.getByTestId("tx-detail-facts")).toBeVisible();
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveCount(0);
    await expect(page.getByTestId("tx-detail-value-now")).toBeVisible();
    await page.keyboard.press("Escape");

    // Stamped in euros before the switch to dollars: it keeps its own currency.
    await page.getByTestId(rowId(sent)).click();
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveText(`${fiatText(sent.amount, "41.2500", "EUR", "en-US")} when sent`);
    await expect(page.getByTestId("tx-detail-value-now-text")).toHaveText(/^\$/);

    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveCount(0);
    await expect(page.getByTestId("tx-detail-value-now")).toHaveCount(0);
  });

  test("prices off hide the stamp too; unavailable prices keep it, with no value now", async ({ page }) => {
    await openActivity(page, "fiat-off");
    const [received] = await rows(page);
    expect(received.fiatThen).toBeTruthy();
    await page.getByTestId(rowId(received)).click();
    await expect(page.getByTestId("tx-detail-facts")).toBeVisible();
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveCount(0);
    await expect(page.getByTestId("tx-detail-value-now")).toHaveCount(0);

    await openActivity(page, "fiat-unavailable");
    await page.getByTestId(rowId(received)).click();
    await expect(page.getByTestId("tx-detail-fiat-then")).toHaveText(`${fiatText(received.amount, "48.1203", "USD", "en-US")} when received`);
    await expect(page.getByTestId("tx-detail-value-now")).toHaveCount(0);
  });
});
