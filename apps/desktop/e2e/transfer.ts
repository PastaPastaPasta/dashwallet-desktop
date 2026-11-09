import { expect, type Page } from "@playwright/test";
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

const SYNCING = "Shielded is still syncing; try again when it finishes.";
const CONSOLIDATE = /Consolidate your coins first/;

/** The fixture's calls of `command` so far. */
export const callsOf = (page: Page, command: string) => page.evaluate((c) => window.__dwFixture!.calls.filter((x) => x === c).length, command);

/** The purpose of the last grant issued. */
export const lastGrant = (page: Page) => page.evaluate(() => window.__dwFixture!.world.grants.at(-1)?.grant.purpose ?? null);

/** Holds the next call of `command`; `release(page)` lets it answer. */
export async function hold(page: Page, command: string) {
  await page.evaluate((c) => void ((window as unknown as { __release: () => void }).__release = window.__dwFixture!.hold(c as never)), command);
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

const GO = { coreToShielded: "shielded-go", shieldedToCore: "shielded-go", coreToPlatform: "platform-fund-go", platformToCore: "platform-move-go" } as const;
const AMOUNT = { coreToShielded: "0.001", shieldedToCore: "0.5", coreToPlatform: "0.5", platformToCore: "" } as const;

async function run(page: Page, route: TransferRoute) {
  await confirm(page, route, AMOUNT[route]);
  await page.getByTestId(GO[route]).click();
  await authorize(page);
}

/** Max: the field filled from a second quote, the fee line read for that amount. */
async function maxFills(page: Page, quote: string, value: RegExp) {
  const before = await callsOf(page, quote);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-amount")).toHaveValue(value);
  await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about .* · You receive about /);
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  // The whole balance, then the most it leaves room for: one re-quote, no more.
  expect((await callsOf(page, quote)) - before).toBe(2);
}

/** Max refused: the refusal shown, the field left as it was. */
async function maxRefused(page: Page, text: RegExp | string) {
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-fee")).toHaveText(text);
  await expect(page.getByTestId("transfer-fee")).toHaveAttribute("role", "alert");
  await expect(page.getByTestId("transfer-amount")).toHaveValue("");
}

async function inProgress(page: Page, route: TransferRoute, command: string, progress: (p: Page) => Promise<void>) {
  await hold(page, command);
  await run(page, route);
  await progress(page);
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
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough Dash in your wallet: /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  { id: "coreToShielded-max", scenario: "funded", go: async (page) => (await openTransfer(page, "coreToShielded"), await maxFills(page, "shield_quote", /^\d+\.\d+$/)) },
  {
    id: "coreToShielded-max-refused",
    scenario: "transfer-shield-pending",
    go: async (page) => {
      await openTransfer(page, "coreToShielded");
      await maxRefused(page, "An earlier add to Shielded is not finished. Finish it from the Shielded card first.");
      expect(await callsOf(page, "shield_quote")).toBe(1);
    },
  },
  {
    id: "coreToShielded-max-coins",
    scenario: "transfer-many-coins",
    go: async (page) => {
      await openTransfer(page, "coreToShielded");
      await maxRefused(page, CONSOLIDATE);
      expect(await callsOf(page, "shield_quote")).toBe(2);
    },
  },
  {
    id: "coreToShielded-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "coreToShielded", "0.001");
      await expect(page.getByTestId("transfer-route")).toHaveText(/Dash wallet.*Shielded/);
      await expect(page.getByTestId("shielded-quote-total")).toContainText("0.00101");
    },
  },
  { id: "coreToShielded-progress", scenario: "funded", go: (page) => inProgress(page, "coreToShielded", "shield_from_core", (p) => expect(p.getByTestId("shielded-progress")).toBeVisible()) },
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
      // The route opens, and its quote says to finish the lock first.
      await typed(page, "coreToPlatform", "0.5");
      await expect(page.getByTestId("transfer-fee")).toHaveText("An earlier add to Platform is not finished. Finish it from the Platform card first.");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  {
    id: "coreToPlatform-insufficient",
    scenario: "funded",
    go: async (page) => {
      await typed(page, "coreToPlatform", "1000");
      await expect(page.getByTestId("transfer-fee")).toHaveText(/^Not enough Dash in your wallet: /);
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  { id: "coreToPlatform-max", scenario: "funded", go: async (page) => (await openTransfer(page, "coreToPlatform"), await maxFills(page, "platform_fund_quote", /^\d+\.\d+$/)) },
  {
    id: "coreToPlatform-max-refused",
    scenario: "paddr-fund-proof-waiting",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await maxRefused(page, "An earlier add to Platform is not finished. Finish it from the Platform card first.");
      expect(await callsOf(page, "platform_fund_quote")).toBe(1);
    },
  },
  {
    id: "coreToPlatform-max-coins",
    scenario: "transfer-many-coins",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await maxRefused(page, CONSOLIDATE);
      expect(await callsOf(page, "platform_fund_quote")).toBe(2);
    },
  },
  {
    id: "coreToPlatform-confirm",
    scenario: "funded",
    go: async (page) => {
      await confirm(page, "coreToPlatform", "0.5");
      await expect(page.getByTestId("platform-fund-confirm")).toContainText("You receive about");
    },
  },
  { id: "coreToPlatform-progress", scenario: "funded", go: (page) => inProgress(page, "coreToPlatform", "platform_fund", (p) => expect(p.getByTestId("platform-fund-progress")).toBeVisible()) },
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
  { id: "shieldedToCore-progress", scenario: "funded", go: (page) => inProgress(page, "shieldedToCore", "unshield_to_core", (p) => expect(p.getByTestId("shielded-progress")).toBeVisible()) },
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

  // Platform → Dash wallet: its whole balance.
  {
    id: "platformToCore-idle",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-whole")).toHaveText("Platform moves its whole balance.");
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
    },
  },
  {
    id: "platformToCore-insufficient",
    scenario: "platform-dust",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-unavailable")).toContainText("cannot be moved to your Dash wallet right now");
      await expect(page.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  {
    id: "platformToCore-max",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      // Max is the whole balance, fixed on; the amount is the quote's.
      await expect(page.getByTestId("transfer-amount")).toHaveValue(/^0\.4/);
      await expect(page.getByTestId("transfer-max")).toHaveAttribute("aria-pressed", "true");
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

  // The sheet's own rows.
  {
    id: "shieldedPlatform-unsupported",
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-platform").click();
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("Moving straight between Shielded and Platform isn't available on desktop yet. Move through your Dash wallet.");
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
    id: "entry-shielded-card",
    scenario: "funded",
    go: async (page) => {
      const card = page.getByRole("button", { name: "Transfer from Shielded" });
      await expect(card).toHaveAttribute("data-testid", "shielded-card-transfer");
      // No control inside the card's button.
      await expect(card.locator("button, a, input")).toHaveCount(0);
      await card.focus();
    },
  },
  {
    id: "entry-platform-card",
    scenario: "funded",
    go: async (page) => {
      const card = page.getByRole("button", { name: "Transfer from Platform" });
      await expect(card).toHaveAttribute("data-testid", "platform-card-transfer");
      await expect(card.locator("button, a, input")).toHaveCount(0);
      await card.focus();
    },
  },
  {
    id: "receive-shielded",
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("sidebar-receive").click();
      const open = page.getByTestId("shielded-receive-open");
      await expect(open).toHaveAttribute("aria-expanded", "false");
      await open.click();
      await expect(open).toHaveAttribute("aria-expanded", "true");
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
