import { expect, type Page } from "@playwright/test";
import type { MoveStage } from "../src/api/generated/MoveStage";
import { authorize, openTransfer, type TransferRoute } from "./helpers";

// Transfer's states (POLISH 25) on the fixture backend: each route × idle, unavailable, insufficient,
// Max, confirm, in progress, done, failed and its pending lock or answer, and the sheet's own rows.
// transfer.spec.ts runs each with its checks; a state ends where its screenshot is taken.

export interface TransferState {
  /** `<route>-<state>`: the screenshot's name without the theme. */
  id: string;
  scenario: string;
  /** More URL options (`&hold=…`, `&fail=…`). */
  extra?: string;
  go: (page: Page) => Promise<void>;
}

const SYNCING = "Shielded is still syncing. Try again when it finishes.";
/** Dash wallet → Platform reads its quote only on Continue (and Max): its fee line while typing. */
export const FEE_ON_NEXT_STEP = "Fee shown on the next step";
const FEE = /^Fee about .* · You receive about /;

/** The fixture's calls of `command` so far. */
export const callsOf = (page: Page, command: string) => page.evaluate((c) => window.__dwFixture!.calls.filter((x) => x === c).length, command);

/** The purpose of the last grant issued. */
export const lastGrant = (page: Page) => page.evaluate(() => window.__dwFixture!.world.grants.at(-1)?.grant.purpose ?? null);

/** Holds the next call of `command`; `release(page)` lets it answer. */
export async function hold(page: Page, command: string, atMoveStage?: MoveStage) {
  await page.evaluate(({ c, stage }) => void ((window as unknown as { __release: () => void }).__release = window.__dwFixture!.hold(c as never, stage)), { c: command, stage: atMoveStage });
}
export const release = (page: Page) => page.evaluate(() => (window as unknown as { __release: () => void }).__release());

const view = (page: Page, route: string, v: "form" | "steps") => expect(page.getByTestId("transfer-body")).toHaveAttribute("data-view", v).then(() => expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", route));

/** The route's form with `amount` typed and its fee read. */
async function typed(page: Page, route: TransferRoute, amount: string) {
  await openTransfer(page, route);
  await page.getByTestId("transfer-amount").fill(amount);
  await expect(page.getByTestId("transfer-fee")).not.toHaveText(/^$|Working out the fee/);
}

/** Through Continue to the route's own confirmation. */
async function confirm(page: Page, route: TransferRoute, amount: string) {
  if (route === "platformToCore") await openTransfer(page, route);
  else await typed(page, route, amount);
  await page.getByTestId("transfer-continue").click();
  await view(page, route, "steps");
}

const GO = { coreToShielded: "shielded-go", shieldedToCore: "shielded-go", shieldedToPlatform: "shielded-go", coreToPlatform: "platform-fund-go", platformToCore: "platform-move-go" } as const;
const AMOUNT = { coreToShielded: "0.001", shieldedToCore: "0.5", shieldedToPlatform: "0.5", coreToPlatform: "0.5", platformToCore: "" } as const;

/** The fixture's Platform receive address, where Shielded → Platform pays. */
export const PLATFORM_RECEIVE = "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0";

async function run(page: Page, route: TransferRoute) {
  await confirm(page, route, AMOUNT[route]);
  await page.getByTestId(GO[route]).click();
  await authorize(page);
}

/** Every text, field value and label in Transfer's sheet that shows an amount in clear. */
export async function amountsInClear(page: Page): Promise<string[]> {
  const sheet = page.getByTestId("transfer-sheet");
  const seen = [await sheet.innerText()];
  seen.push(...(await sheet.locator("input").evaluateAll((els) => els.map((e) => (e as HTMLInputElement).value))));
  seen.push(...(await sheet.locator("[aria-label]").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") ?? ""))));
  return seen.filter((v) => /\d+[.,]\d+|\d+ ?t?DASH/.test(v));
}

/** A route that can't run reads no quote: Platform → Dash wallet's, while it waits. */
async function noMoveQuote(page: Page) {
  await page.waitForTimeout(500);
  expect(await callsOf(page, "platform_move_to_core_quote")).toBe(0);
}

/** Max: the field filled, the fee line read for that amount (`fee`), from `calls` quotes: from Shielded,
 * the whole balance then the most it leaves room for; from the Dash wallet, one quote of the whole
 * account (`{kind: "all"}`). */
async function maxFills(page: Page, quote: string, value: RegExp, fee: RegExp | string = FEE, calls = 2) {
  const before = await callsOf(page, quote);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-amount")).toHaveValue(value);
  await expect(page.getByTestId("transfer-fee")).toHaveText(fee);
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  expect((await callsOf(page, quote)) - before).toBe(calls);
}

/** Max refused because the account has more than 50 coins. */
const MAX_TOO_MANY = /^Max needs at most 50 coins; this wallet has \d+\. Enter an amount instead\.$/;

/** A Dash wallet Max through Continue: the confirmation names everything it moves and its fee, and
 * each amount it leaves behind (`notes`), none at 0. */
async function maxConfirm(page: Page, route: "coreToShielded" | "coreToPlatform", notes: RegExp[]) {
  await openTransfer(page, route);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  await page.getByTestId("transfer-continue").click();
  await view(page, route, "steps");
  const to = route === "coreToShielded" ? "Shielded" : "your Platform address";
  await expect(page.getByTestId("core-max-confirm").locator(".sheet-text")).toHaveText(new RegExp(`^Move .+ from your Dash wallet to ${to}\\? That's everything Max can move\\. The network fee \\(about .+\\) comes out of it\\.$`));
  await expect(page.getByTestId("core-max-note")).toHaveText(notes);
  await expect(page.getByTestId(GO[route])).toBeEnabled();
}

const LEFT = /^.+ stays in your Dash wallet, because Max doesn't include mixed coins, payments from contacts or coins in other accounts\.$/;
const ARRIVING = /^.+ still arriving stays in your Dash wallet\. Move it once it confirms\.$/;

/** Max from the Dash wallet below the route's smallest amount (`min`): it names what Max can move (the
 * one coin of 0.00005000 less the lock's fee bound of 0.00001148), and only when a coin stays in the
 * wallet's other accounts (`left`), what Max leaves out. No line of what stays shows beside it. */
async function maxBelowMinimum(page: Page, route: "coreToShielded" | "coreToPlatform", min: string, left: boolean) {
  await openTransfer(page, route);
  const below = `Max can move only 0.00003852 tDASH, which is less than the smallest amount, ${min} tDASH.`;
  await maxRefused(page, left ? `${below} Max doesn't include mixed coins, payments from contacts or coins in other accounts.` : below);
  await expect(page.getByTestId("transfer-sheet").getByText(/stays in your Dash wallet|Enter an amount instead/)).toHaveCount(0);
  expect(await callsOf(page, route === "coreToShielded" ? "shield_quote" : "platform_fund_quote")).toBe(1);
}

/** Max from the Dash wallet with nothing to move (`text`): its coins still arriving, every coin one Max
 * leaves out, or, with neither, nothing to move at all. The refusal never says 0.00000000 or the
 * smallest amount, and no line of what stays shows beside it. */
async function maxZero(page: Page, route: "coreToShielded" | "coreToPlatform", text: string) {
  await openTransfer(page, route);
  await maxRefused(page, text);
  await expect(page.getByTestId("transfer-fee")).not.toContainText(/0\.00000000|Too small/);
  await expect(page.getByTestId("transfer-sheet").getByText(/stays in your Dash wallet|Enter an amount instead/)).toHaveCount(0);
  expect(await callsOf(page, route === "coreToShielded" ? "shield_quote" : "platform_fund_quote")).toBe(1);
}

/** Dash wallet → Platform with only a payment still arriving: an amount typed first is more than can
 * be spent, then Max's refusal says why it moved nothing, ahead of that, until the amount is edited.
 * Continue stays off throughout. */
async function maxAfterTyped(page: Page) {
  await openTransfer(page, "coreToPlatform");
  const amount = page.getByTestId("transfer-amount");
  const fee = page.getByTestId("transfer-fee");
  await amount.fill("0.001");
  await expect(fee).toHaveText(/^Not enough spendable Dash: /);
  await amount.fill("0.0010");
  await expect(fee).toHaveText(/^Not enough spendable Dash: /);
  await page.getByTestId("transfer-max").click();
  await expect(fee).toHaveText(MAX_ALL_ARRIVING);
  await expect(page.getByTestId("transfer-fee")).toHaveClass(/form-problem/);
  await expect(amount).toHaveValue("0.0010");
  await expect(page.getByTestId("transfer-continue")).toBeDisabled();
  await amount.fill("0.001");
  await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough spendable Dash: /);
  await expect(page.getByTestId("transfer-continue")).toBeDisabled();
}

async function secondArrival(page: Page, route: "coreToShielded" | "coreToPlatform") {
  await maxZero(page, route, MAX_ARRIVING_TOO_SMALL);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const w = f.world.wallets[0];
    const first = w.coins[0];
    w.coins = [...w.coins, { ...first, amount: 195_000, outpoint: { ...first.outpoint, vout: first.outpoint.vout + 1 } }];
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.summary.walletId, balances: w.summary.balances });
  });
  await expect(page.getByTestId("transfer-fee")).not.toHaveText(MAX_ARRIVING_TOO_SMALL);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-fee")).toHaveText(MAX_ALL_ARRIVING);
}

const MAX_ARRIVING_LOCKED = "Max can't run: some coins in your Dash wallet are locked. Enter an amount instead.";
const maxArrivingCantMove = (amount: string) => `Your Dash wallet's coins are still arriving (${amount} tDASH), but Max can't move them as they are, even once they've confirmed.`;

const MAX_ALL_ARRIVING = "Your Dash wallet's coins are still arriving (0.00200000 tDASH). Max can move them once they've confirmed.";
const MAX_ARRIVING_TOO_SMALL = "Your Dash wallet's coins are still arriving (0.00005000 tDASH), but even once they've confirmed, that's less than the smallest amount this transfer can move.";
const MAX_NOTHING = "Max has nothing to move: it doesn't include mixed coins, payments from contacts or coins in other accounts.";
const MAX_EMPTY = "There's nothing in your Dash wallet to move.";
const NO_CORE_BALANCE = "Nothing in your Dash wallet to move yet.";

/** No coin a Max could take in the Dash wallet: Max is disabled, the line under the amount (not beside
 * Max, where it squeezed the field) says there is nothing to move, nothing is quoted, no refusal. */
async function maxNone(page: Page, route: "coreToShielded" | "coreToPlatform") {
  await openTransfer(page, route);
  await expect(page.getByTestId("transfer-max")).toBeDisabled();
  await expect(page.getByTestId("transfer-sheet").locator(".transfer-amount-row .dash-button-why")).toHaveCount(0);
  await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
  await expect(page.getByTestId("transfer-amount")).toBeEnabled();
  await expect(page.getByTestId("transfer-fee")).toHaveClass(/field-hint/);
  await expect(page.getByTestId("transfer-fee")).toHaveText(NO_CORE_BALANCE);
  expect(await callsOf(page, route === "coreToShielded" ? "shield_quote" : "platform_fund_quote")).toBe(0);
}

/** Max refused: the refusal shown, the field left as it was. */
async function maxRefused(page: Page, text: RegExp | string) {
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-fee")).toHaveText(text);
  // One live region for the fee line, the refusal in it; Continue's reason is that refusal, not the
  // generic hint.
  await expect(page.getByTestId("transfer-fee")).toHaveAttribute("role", "status");
  await expect(page.getByTestId("transfer-amount")).toHaveValue("");
  await expect(page.getByTestId("transfer-sheet").getByText("Enter an amount first.")).toHaveCount(0);
  await expect(page.getByTestId("transfer-continue")).toHaveAttribute("aria-describedby", (await page.getByTestId("transfer-fee").getAttribute("id"))!);
}

async function inProgress(page: Page, route: TransferRoute, command: string, progress: (p: Page) => Promise<void>) {
  await hold(page, command);
  await run(page, route);
  await progress(page);
}

const MOVE_COMMAND = { coreToShielded: "shield_from_core", shieldedToCore: "unshield_to_core", shieldedToPlatform: "unshield_to_platform" };

export async function moveProgressAt(page: Page, route: keyof typeof MOVE_COMMAND, stage: MoveStage) {
  await hold(page, MOVE_COMMAND[route], stage);
  await run(page, route);
  await expect(page.getByTestId("shielded-progress").locator('[aria-current="step"]')).toHaveAttribute("data-stage", stage);
}

function arrivingStates(route: "coreToShielded" | "coreToPlatform"): TransferState[] {
  return [
    ...(["locked", "locked-with-mixed"] as const).map((state) => ({
      id: `${route}-max-arriving-${state}`,
      scenario: `transfer-max-arriving-${state}`,
      go: async (page: Page) => {
        await openTransfer(page, route);
        await maxRefused(page, MAX_ARRIVING_LOCKED);
      },
    })),
    { id: `${route}-max-arriving-cap`, scenario: "transfer-max-arriving-cap", go: (page) => maxZero(page, route, maxArrivingCantMove("0.10200000")) },
    { id: `${route}-max-arriving-contact`, scenario: "transfer-max-arriving-contact", go: (page) => maxZero(page, route, maxArrivingCantMove("0.00200000")) },
    { id: `${route}-max-second-arrival`, scenario: "transfer-max-incoming-small", go: (page) => secondArrival(page, route) },
  ];
}
export const TRANSFER_STATES: TransferState[] = [
  // Dash wallet → Shielded (the default route).
  {
    id: "coreToShielded-idle",
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await view(page, "coreToShielded", "form");
      await expect(page.getByTestId("transfer-from")).toHaveAttribute("data-pool", "core");
      await expect(page.getByTestId("transfer-to")).toHaveAttribute("data-pool", "shielded");
      await expect(page.getByTestId("transfer-amount")).toBeFocused();
      await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
      await expect(page.getByTestId("transfer-fee")).toHaveText("");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  {
    id: "coreToShielded-unavailable",
    scenario: "shielded-syncing",
    go: async (page) => {
      await openTransfer(page, "coreToShielded");
      await expect(page.getByTestId("transfer-unavailable")).toHaveText(SYNCING);
      await expect(page.getByTestId("transfer-amount")).toBeDisabled();
      await expect(page.getByTestId("transfer-max")).toBeDisabled();
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      expect(await callsOf(page, "shield_quote")).toBe(0);
    },
  },
  {
    id: "coreToShielded-insufficient",
    scenario: "funded",
    go: async (page) => {
      await typed(page, "coreToShielded", "1000");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough (?:Dash in your wallet|spendable Dash): /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  { id: "coreToShielded-max", scenario: "funded", go: async (page) => (await openTransfer(page, "coreToShielded"), await maxFills(page, "shield_quote", /^\d+\.\d+$/, FEE, 1)) },
  { id: "coreToShielded-max-confirm", scenario: "funded", go: (page) => maxConfirm(page, "coreToShielded", [ARRIVING]) },
  { id: "coreToShielded-max-confirm-left", scenario: "coinjoin-move", go: (page) => maxConfirm(page, "coreToShielded", [LEFT, ARRIVING]) },
  {
    id: "coreToShielded-max-refused",
    scenario: "transfer-shield-pending",
    go: async (page) => {
      await openTransfer(page, "coreToShielded");
      await maxRefused(page, "An earlier add to Shielded is not finished. Finish it from Unfinished transfers on Home first.");
      expect(await callsOf(page, "shield_quote")).toBe(1);
    },
  },
  {
    id: "coreToShielded-max-coins",
    scenario: "transfer-many-coins",
    go: async (page) => {
      await openTransfer(page, "coreToShielded");
      await maxRefused(page, MAX_TOO_MANY);
      expect(await callsOf(page, "shield_quote")).toBe(1);
    },
  },
  { id: "coreToShielded-max-below-minimum", scenario: "transfer-max-dust", go: (page) => maxBelowMinimum(page, "coreToShielded", "0.00010001", false) },
  { id: "coreToShielded-max-below-minimum-left", scenario: "transfer-max-dust-left", go: (page) => maxBelowMinimum(page, "coreToShielded", "0.00010001", true) },
  { id: "coreToShielded-max-all-arriving", scenario: "transfer-max-arriving", go: (page) => maxZero(page, "coreToShielded", MAX_ALL_ARRIVING) },
  { id: "coreToShielded-max-arriving-too-small", scenario: "transfer-max-arriving-small", go: (page) => maxZero(page, "coreToShielded", MAX_ARRIVING_TOO_SMALL) },
  ...arrivingStates("coreToShielded"),
  { id: "coreToShielded-max-incoming", scenario: "transfer-max-incoming", go: (page) => maxZero(page, "coreToShielded", MAX_ALL_ARRIVING) },
  { id: "coreToShielded-max-mixed-only", scenario: "transfer-max-mixed-only", go: (page) => maxZero(page, "coreToShielded", MAX_NOTHING) },
  { id: "coreToShielded-max-nothing", scenario: "transfer-max-nothing", go: (page) => maxZero(page, "coreToShielded", MAX_NOTHING) },
  { id: "coreToShielded-max-empty", scenario: "transfer-max-tiny", go: (page) => maxZero(page, "coreToShielded", MAX_EMPTY) },
  { id: "coreToShielded-max-none", scenario: "transfer-max-none", go: (page) => maxNone(page, "coreToShielded") },
  { id: "coreToShielded-max-locked-only", scenario: "transfer-max-locked-only", go: (page) => maxNone(page, "coreToShielded") },
  {
    id: "coreToShielded-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "coreToShielded", "0.001");
      await expect(page.getByTestId("transfer-route")).toHaveText(/Dash wallet.*Shielded/);
      await expect(page.getByTestId("shielded-quote-total")).toContainText("0.00101");
    },
  },
  { id: "coreToShielded-progress", scenario: "funded", go: (page) => moveProgressAt(page, "coreToShielded", "coreAssetLock") },
  {
    id: "coreToShielded-done",
    scenario: "funded",
    go: async (page) => {
      await run(page, "coreToShielded");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    },
  },
  {
    id: "coreToShielded-failed",
    scenario: "transfer-shielded-refused",
    go: async (page) => {
      await run(page, "coreToShielded");
      await expect(page.getByTestId("shielded-refusal")).toBeVisible();
    },
  },
  {
    id: "coreToShielded-pending",
    scenario: "transfer-shield-waiting",
    go: async (page) => {
      await run(page, "coreToShielded");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Locked, not applied yet");
    },
  },

  // Dash wallet → Platform.
  {
    id: "coreToPlatform-idle",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await expect(page.getByTestId("platform-fund-intro")).toBeVisible();
      await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
    },
  },
  {
    // Dash wallet → Platform is only unavailable with no Platform addresses, where it is not listed
    // (none-absent, none-inactive). An earlier lock not applied yet holds the next add back instead.
    id: "coreToPlatform-earlier-lock",
    scenario: "paddr-fund-proof-waiting",
    go: async (page) => {
      // The route opens; its quote, read on Continue, says to finish the lock first.
      await typed(page, "coreToPlatform", "0.5");
      await expect(page.getByTestId("transfer-fee")).toHaveText(FEE_ON_NEXT_STEP);
      await page.getByTestId("transfer-continue").click();
      await expect(page.getByTestId("platform-fund-quote-error")).toHaveText("An earlier add to Platform is not finished. Finish it from Unfinished transfers on Home first.");
      await expect(page.getByTestId("platform-fund-go")).toBeDisabled();
    },
  },
  {
    id: "coreToPlatform-insufficient",
    scenario: "funded",
    go: async (page) => {
      await typed(page, "coreToPlatform", "1000");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough spendable Dash: /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      expect(await callsOf(page, "platform_fund_quote")).toBe(0);
    },
  },
  { id: "coreToPlatform-max", scenario: "funded", go: async (page) => (await openTransfer(page, "coreToPlatform"), await maxFills(page, "platform_fund_quote", /^\d+\.\d+$/, FEE_ON_NEXT_STEP, 1)) },
  { id: "coreToPlatform-max-confirm", scenario: "funded", go: (page) => maxConfirm(page, "coreToPlatform", [ARRIVING]) },
  { id: "coreToPlatform-max-confirm-left", scenario: "coinjoin-move", go: (page) => maxConfirm(page, "coreToPlatform", [LEFT, ARRIVING]) },
  {
    id: "coreToPlatform-max-refused",
    scenario: "paddr-fund-proof-waiting",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await maxRefused(page, "An earlier add to Platform is not finished. Finish it from Unfinished transfers on Home first.");
      expect(await callsOf(page, "platform_fund_quote")).toBe(1);
    },
  },
  {
    id: "coreToPlatform-max-coins",
    scenario: "transfer-many-coins",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await maxRefused(page, MAX_TOO_MANY);
      expect(await callsOf(page, "platform_fund_quote")).toBe(1);
    },
  },
  { id: "coreToPlatform-max-below-minimum", scenario: "transfer-max-dust", go: (page) => maxBelowMinimum(page, "coreToPlatform", "0.00056001", false) },
  { id: "coreToPlatform-max-below-minimum-left", scenario: "transfer-max-dust-left", go: (page) => maxBelowMinimum(page, "coreToPlatform", "0.00056001", true) },
  { id: "coreToPlatform-max-all-arriving", scenario: "transfer-max-arriving", go: (page) => maxZero(page, "coreToPlatform", MAX_ALL_ARRIVING) },
  { id: "coreToPlatform-max-arriving-too-small", scenario: "transfer-max-arriving-small", go: (page) => maxZero(page, "coreToPlatform", MAX_ARRIVING_TOO_SMALL) },
  ...arrivingStates("coreToPlatform"),
  { id: "coreToPlatform-max-incoming", scenario: "transfer-max-incoming", go: (page) => maxZero(page, "coreToPlatform", MAX_ALL_ARRIVING) },
  { id: "coreToPlatform-max-incoming-typed", scenario: "transfer-max-incoming", go: (page) => maxAfterTyped(page) },
  { id: "coreToPlatform-max-mixed-only", scenario: "transfer-max-mixed-only", go: (page) => maxZero(page, "coreToPlatform", MAX_NOTHING) },
  { id: "coreToPlatform-max-nothing", scenario: "transfer-max-nothing", go: (page) => maxZero(page, "coreToPlatform", MAX_NOTHING) },
  { id: "coreToPlatform-max-empty", scenario: "transfer-max-tiny", go: (page) => maxZero(page, "coreToPlatform", MAX_EMPTY) },
  { id: "coreToPlatform-max-none", scenario: "transfer-max-none", go: (page) => maxNone(page, "coreToPlatform") },
  { id: "coreToPlatform-max-locked-only", scenario: "transfer-max-locked-only", go: (page) => maxNone(page, "coreToPlatform") },
  {
    id: "coreToPlatform-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "coreToPlatform", "0.5");
      await expect(page.getByTestId("platform-fund-confirm")).toContainText("You receive about");
    },
  },
  { id: "coreToPlatform-progress", scenario: "funded", go: (page) => inProgress(page, "coreToPlatform", "platform_fund", (p) => expect(p.getByTestId("platform-fund-progress")).toBeVisible().then(() => expect(p.getByTestId("transfer-preparing")).toBeVisible())) },
  {
    id: "coreToPlatform-done",
    scenario: "funded",
    go: async (page) => {
      await run(page, "coreToPlatform");
      await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
    },
  },
  {
    id: "coreToPlatform-failed",
    scenario: "transfer-fund-not-applied",
    go: async (page) => {
      await run(page, "coreToPlatform");
      await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Locked, not applied");
    },
  },
  {
    id: "coreToPlatform-pending",
    scenario: "transfer-fund-waiting",
    go: async (page) => {
      await run(page, "coreToPlatform");
      await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Locked, not applied yet");
    },
  },

  // Shielded → Dash wallet.
  {
    id: "shieldedToCore-idle",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await expect(page.getByTestId("transfer-from")).toHaveAttribute("data-pool", "shielded");
      await expect(page.getByTestId("transfer-to")).toHaveAttribute("data-pool", "core");
    },
  },
  {
    id: "shieldedToCore-unavailable",
    scenario: "shielded-empty",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await expect(page.getByTestId("transfer-unavailable")).toContainText("Nothing in Shielded to send yet.");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      expect(await callsOf(page, "unshield_quote")).toBe(0);
    },
  },
  {
    id: "shieldedToCore-insufficient",
    scenario: "funded",
    go: async (page) => {
      await typed(page, "shieldedToCore", "5");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough in Shielded: /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  { id: "shieldedToCore-max", scenario: "funded", go: async (page) => (await openTransfer(page, "shieldedToCore"), await maxFills(page, "unshield_quote", /^1\.24980*$/)) },
  {
    id: "shieldedToCore-max-refused",
    scenario: "funded",
    extra: "&fail=unshield_quote:none",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await maxRefused(page, /.+/);
      expect(await callsOf(page, "unshield_quote")).toBe(1);
    },
  },
  {
    id: "shieldedToCore-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "shieldedToCore", "0.5");
      await expect(page.getByTestId("shielded-quote-receive")).toContainText("0.5");
    },
  },
  { id: "shieldedToCore-progress", scenario: "funded", go: (page) => moveProgressAt(page, "shieldedToCore", "shieldedProof") },
  {
    id: "shieldedToCore-done",
    scenario: "funded",
    go: async (page) => {
      await run(page, "shieldedToCore");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
    },
  },
  {
    id: "shieldedToCore-failed",
    scenario: "transfer-shielded-refused",
    go: async (page) => {
      await run(page, "shieldedToCore");
      await expect(page.getByTestId("shielded-refusal")).toBeVisible();
    },
  },
  {
    id: "shieldedToCore-pending",
    scenario: "funded",
    go: async (page) => {
      await page.evaluate(() => void (window.__dwFixture!.world.pending.shielded!.next = "unknown"));
      await run(page, "shieldedToCore");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Not confirmed yet");
    },
  },

  // Shielded → Platform: to the wallet's own Platform receive address.
  {
    id: "shieldedToPlatform-idle",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "shieldedToPlatform");
      await view(page, "shieldedToPlatform", "form");
      await expect(page.getByTestId("transfer-from")).toHaveAttribute("data-pool", "shielded");
      await expect(page.getByTestId("transfer-to")).toHaveAttribute("data-pool", "platform");
      await expect(page.getByTestId("transfer-body")).toContainText("Move Shielded funds to your Platform address.");
      await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
      // Platform → Shielded has no command: no swap.
      await expect(page.getByTestId("transfer-swap")).toBeDisabled();
    },
  },
  {
    id: "shieldedToPlatform-unavailable",
    scenario: "shielded-empty",
    go: async (page) => {
      await openTransfer(page, "shieldedToPlatform");
      await expect(page.getByTestId("transfer-unavailable")).toContainText("Nothing in Shielded to send yet.");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      expect(await callsOf(page, "unshield_to_platform_quote")).toBe(0);
    },
  },
  {
    id: "shieldedToPlatform-insufficient",
    scenario: "funded",
    go: async (page) => {
      await typed(page, "shieldedToPlatform", "5");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough in Shielded: /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  {
    id: "shieldedToPlatform-max",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "shieldedToPlatform");
      await maxFills(page, "unshield_to_platform_quote", /^1\.249739170*$/);
      expect(await callsOf(page, "unshield_quote")).toBe(0);
    },
  },
  {
    id: "shieldedToPlatform-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "shieldedToPlatform", "0.5");
      await expect(page.getByTestId("shielded-quote-amount")).toContainText("0.5");
      // Credits, not duffs: nothing reaches a Core address.
      await expect(page.getByTestId("shielded-quote-receive")).toHaveCount(0);
      await expect(page.getByTestId("shielded-slow")).toBeVisible();
    },
  },
  { id: "shieldedToPlatform-progress", scenario: "funded", go: (page) => moveProgressAt(page, "shieldedToPlatform", "shieldedProof") },
  {
    id: "shieldedToPlatform-done",
    scenario: "funded",
    go: async (page) => {
      await run(page, "shieldedToPlatform");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
      await expect(page.getByTestId("shielded-outcome-text")).toHaveText(`Moved to your Platform address ${PLATFORM_RECEIVE}.`);
    },
  },
  {
    id: "shieldedToPlatform-failed",
    scenario: "transfer-shielded-refused",
    go: async (page) => {
      await run(page, "shieldedToPlatform");
      await expect(page.getByTestId("shielded-refusal")).toBeVisible();
    },
  },
  {
    id: "shieldedToPlatform-may-be-out",
    scenario: "funded",
    go: async (page) => {
      await page.evaluate(() => window.__dwFixture!.fail("unshield_to_platform", "artifact"));
      await run(page, "shieldedToPlatform");
      await expect(page.getByTestId("shielded-progress").locator('[data-state="failed"]')).toHaveAttribute("data-stage", "platformSubmission");
      await expect(page.getByTestId("shielded-refusal")).toHaveText("This move may still go through. Until Dash Wallet knows, these Shielded funds stay held. Don't move them again until Activity shows this move as done or failed.");
      await expect(page.getByTestId("shielded-refused-back")).toHaveCount(0);
    },
  },
  {
    id: "shieldedToPlatform-pending",
    scenario: "funded",
    go: async (page) => {
      await page.evaluate(() => void (window.__dwFixture!.world.pending.shielded!.next = "unknown"));
      await run(page, "shieldedToPlatform");
      await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Not confirmed yet");
      // No retry: the outcome's only action is Close.
      await expect(page.getByTestId("shielded-body").getByRole("button")).toHaveText(["Close"]);
    },
  },
  {
    id: "shieldedToPlatform-mainnet",
    scenario: "transfer-mainnet",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-platform").click();
      await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "unsupported");
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("This version can move from Shielded to Platform on test networks only.");
      await expect(page.getByTestId("transfer-unavailable")).toHaveClass("field-hint");
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      expect(await callsOf(page, "unshield_to_platform_quote")).toBe(0);
      // The other routes still run on mainnet.
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-core").click();
      await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "shieldedToCore");
    },
  },

  // Platform → Dash wallet: its whole balance.
  {
    id: "platformToCore-idle",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      // No address shows here: Confirm names it.
      await expect(page.getByTestId("transfer-body")).toContainText("Dash Platform pays it to an address in your Dash wallet.");
      await expect(page.getByTestId("transfer-body")).not.toContainText("address below");
      await expect(page.getByTestId("transfer-whole")).toHaveText("This moves the spendable balance on your Platform addresses.");
      await expect(page.getByTestId("transfer-amount")).toHaveAttribute("readonly", "");
      await expect(page.getByTestId("transfer-max")).toHaveAttribute("aria-pressed", "true");
      await expect(page.getByTestId("transfer-max")).toBeDisabled();
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about .* · You receive about /);
      await expect(page.getByTestId("transfer-continue")).toBeEnabled();
    },
  },
  {
    id: "platformToCore-unavailable",
    scenario: "platform-empty",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("Nothing on your Platform addresses to move yet.");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      await noMoveQuote(page);
    },
  },
  {
    id: "platformToCore-insufficient",
    scenario: "platform-dust",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-unavailable")).toContainText("cannot be moved to your Dash wallet right now");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
      await noMoveQuote(page);
    },
  },
  {
    id: "platformToCore-max",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      // Max is the whole balance, fixed on; the amount is the quote's.
      await expect(page.getByTestId("transfer-amount")).toHaveValue("0.39998000");
      await expect(page.getByTestId("transfer-max")).toHaveAttribute("aria-pressed", "true");
    },
  },
  {
    // Discreet mode: the whole balance the route moves, masked like every other figure on the sheet.
    id: "platformToCore-discreet",
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("toolbar-discreet").click();
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about #/);
      await expect(page.getByTestId("transfer-amount")).toHaveValue(/^[#.,]+$/);
      expect(await amountsInClear(page)).toEqual([]);
    },
  },
  {
    id: "platformToCore-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "platformToCore", "");
      await expect(page.getByTestId("platform-move-quote-duffs")).toContainText("0.39998");
      // One quote: the form's, kept for the confirmation.
      expect(await callsOf(page, "platform_move_to_core_quote")).toBe(1);
      // The confirmation shows the address, and says so.
      await expect(page.getByTestId("platform-move-body")).toContainText("Dash Platform pays it to the address below.");
    },
  },
  {
    id: "platformToCore-progress",
    scenario: "funded",
    go: (page) => inProgress(page, "platformToCore", "platform_move_to_core", (p) => expect(p.getByTestId("platform-move-body")).toContainText("Moving…")),
  },
  {
    id: "platformToCore-done",
    scenario: "funded",
    go: async (page) => {
      await run(page, "platformToCore");
      await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
    },
  },
  {
    id: "platformToCore-failed",
    scenario: "transfer-move-refused",
    go: async (page) => {
      await run(page, "platformToCore");
      await expect(page.getByTestId("platform-move-refusal")).toBeVisible();
    },
  },
  {
    id: "platformToCore-pending",
    scenario: "transfer-move-pending",
    go: async (page) => {
      await run(page, "platformToCore");
      await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    },
  },

  {
    id: "platformShielded-mainnet",
    scenario: "transfer-mainnet",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await page.getByTestId("transfer-from").click();
      await page.getByTestId("transfer-picker-platform").click();
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-shielded").click();
      await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "unsupported");
      // No detour through the Dash wallet: Platform to Dash wallet is off on mainnet too.
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("Moving from Platform to Shielded isn't available in this version.");
      await expect(page.getByTestId("transfer-unavailable")).toHaveClass("field-hint");
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
    },
  },

  // The sheet's own rows.
  {
    id: "platformShielded-unsupported",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-shielded").click();
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("Moving from Platform to Shielded isn't available on desktop yet. Move the funds to your Dash wallet first, then to Shielded.");
      // Information, not a failure: the hint style the other unavailable lines use.
      await expect(page.getByTestId("transfer-unavailable")).toHaveClass("field-hint");
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      // Not a dead end: the picker still picks.
      await page.getByTestId("transfer-from").click();
      await expect(page.getByTestId("transfer-picker")).toBeVisible();
    },
  },
  {
    id: "none-absent",
    scenario: "pools-absent",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-unavailable")).toHaveText(/neither Shielded nor Platform addresses/);
      await expect(page.getByTestId("transfer-from")).toHaveCount(0);
    },
  },
  {
    id: "none-inactive",
    scenario: "transfer-none",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-unavailable")).toHaveText(/Shielded is not active on this network yet/);
    },
  },
  {
    id: "running-line",
    scenario: "funded",
    go: async (page) => {
      await inProgress(page, "coreToShielded", "shield_from_core", (p) => expect(p.getByTestId("shielded-progress")).toBeVisible());
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-running")).toHaveText(/^Running: Add to Shielded\./);
      await expect(page.getByTestId("transfer-running")).toHaveAttribute("data-route", "coreToShielded");
    },
  },
  {
    // The shortcut before Shielded and the Platform addresses are read: a loading line, no route yet.
    id: "loading",
    scenario: "funded",
    extra: "&hold=shielded_status&hold=platform_addresses",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-loading")).toHaveText("Reading your balances…");
      await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
    },
  },
  {
    id: "home-without-shielded-card",
    scenario: "funded",
    go: async (page) => {
      await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
      await expect(page.getByTestId("shielded-card")).toHaveCount(0);
    },
  },
  {
    id: "home-without-platform-card",
    scenario: "funded",
    go: async (page) => {
      await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
      await expect(page.getByTestId("platform-card-transfer")).toHaveCount(0);
      await expect(page.getByTestId("platform-card")).toHaveCount(0);
    },
  },
  {
    id: "receive-shielded",
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("sidebar-receive").click();
      const open = page.getByTestId("shielded-receive-open");
      await expect(open).toHaveAttribute("aria-pressed", "false");
      await open.click();
      await expect(open).toHaveAttribute("aria-pressed", "true");
      await expect(page.getByTestId("shielded-receive-address")).toContainText("tdash1zq0k8y");
    },
  },
  {
    id: "pay-shielded",
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("sidebar-pay").click();
      await expect(page.getByTestId("shielded-send")).toHaveText("Send from Shielded");
      await expect(page.getByTestId("shielded-send")).toBeEnabled();
    },
  },
];
