import { expect, test, type Page } from "@playwright/test";
import { CLUB, HOME, OFFICE } from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// MG-10 S1: Masternodes ▸ My masternodes on the fixture: the list, the read-only pane, rename and
// copy, the payout headings into Activity's filters, the states, and the unlock re-read.

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) } });
  });
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

const row = (page: Page, hash: string) => page.getByTestId(`mn-row-${hash}`);
const pane = (page: Page) => page.getByTestId("mn-pane");

for (const theme of THEMES) {
  test(`my masternodes ${theme}: groups, summary and pane; 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "mn-list", theme);
    await expect(page.getByTestId("masternodes-tabs-myMasternodes")).toHaveAttribute("aria-selected", "true");
    await expect(page.getByTestId("mn-list-height")).toHaveText("Masternode list at block 1,214,880");
    await expect(page.getByTestId("mn-group-attention")).toContainText("Office node");
    await expect(page.getByTestId("mn-group-mine")).toContainText("Home node");
    await expect(page.getByTestId("mn-group-tracked")).toBeVisible();
    await expect(page.getByTestId("mn-group-retired")).toContainText("Retired (1)");
    await expect(page.getByTestId("mn-summary")).toContainText("2 yours");
    // The first row is selected: Office, banned, with MG-00's notice; its owner key is here, so
    // Change keys or payout is offered and the notice names it.
    await expect(row(page, OFFICE)).toHaveAttribute("aria-current", "true");
    await expect(page.getByTestId("mn-banned")).toContainText("This masternode is banned and earns nothing.");
    await expect(page.getByTestId("mn-banned")).toContainText("To move to another operator, use Change keys or payout.");
    await expect(page.getByTestId("mn-change-keys")).toBeVisible();
    await expect(page.getByTestId("mn-vote")).toBeDisabled();
    await expect(pane(page)).toContainText("A banned masternode can't vote");
    await expect(page.getByTestId("sidebar-masternodes-badge")).toHaveText("1");
    // S2 adds Set up a masternode and In progress (none here); S3 adds Change keys.
    await expect(page.getByTestId("mn-set-up")).toHaveText("Set up a masternode");
    for (const hidden of ["Track a masternode", "Withdraw", "In progress", "Masternode keys", "Keys you added", "Add keys"])
      await expect(page.getByText(hidden, { exact: false })).toHaveCount(0);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("list → pane → rename → copy", async ({ page }) => {
  const errors = await open(page, "mn-list");
  await row(page, HOME).click();
  await expect(page.getByTestId("mn-pane-title")).toHaveText("Home node");
  await expect(page.getByTestId("mn-fact-service")).toHaveText("203.0.113.7:19999");
  await expect(page.getByTestId("mn-vote")).toBeEnabled();
  await page.getByTestId("mn-more").click();
  await expect(page.getByRole("menuitem")).toHaveText(["Rename…", "Copy ProTx hash", "Copy collateral outpoint", "Release collateral…", "Service update or revoke…"]);
  await page.getByTestId("mn-more-rename").click();
  await page.getByTestId("mn-rename-input").fill("Basement node");
  await page.getByTestId("mn-rename-save").click();
  await expect(page.getByTestId("mn-rename-sheet")).toHaveCount(0);
  await expect(row(page, HOME)).toContainText("Basement node");
  await expect(page.getByTestId("mn-pane-title")).toHaveText("Basement node");
  await page.getByTestId("mn-more").click();
  await page.getByTestId("mn-more-copy-hash").click();
  expect(await page.evaluate(() => (window as unknown as { __copied: string }).__copied)).toBe(HOME);
  // A tracked evonode's collateral isn't resolved: no outpoint to copy.
  await row(page, CLUB).click();
  await page.getByTestId("mn-more").click();
  await expect(page.getByRole("menuitem")).toHaveText(["Rename…", "Copy ProTx hash", "Service update or revoke…"]);
  await page.keyboard.press("Escape");
  await row(page, HOME).click();
  // Vote goes to Governance, filtered to what needs a vote.
  await page.getByTestId("mn-vote").click();
  await expect(page.getByTestId("masternodes-tabs-governance")).toHaveAttribute("aria-selected", "true");
  expect(errors).toEqual([]);
});

const paymentsIn = async (page: Page, id: string) => Number((await page.getByTestId(id).locator("p").first().innerText()).match(/in (\d+) payment/)![1]);

test("flagged payouts heading → See in Activity, per node and Generated", async ({ page }) => {
  const errors = await open(page, "mn-v24-payouts");
  await row(page, HOME).click();
  const flagged = page.getByTestId("mn-payouts-flagged");
  await expect(flagged).toContainText("Payments to this payout address (may include shared-masternode payouts)");
  const n = await paymentsIn(page, "mn-payouts-flagged");
  await page.getByTestId("mn-flagged-activity").click();
  await expect(page.getByTestId("page-activity")).toBeVisible();
  await expect(page.getByTestId("tx-route-filter")).toContainText("Payments to Home node's payout address");
  await expect(page.getByTestId("tx-footer")).toHaveText(n === 1 ? "1 transaction" : `${n} transactions`);
  await page.getByTestId("tx-route-filter").click();
  await expect(page.getByTestId("tx-route-filter")).toHaveCount(0);

  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("mn-summary-unattributed")).toBeVisible();
  await expect(page.getByTestId("mn-summary-payouts")).toContainText("The wallet can't match these payments to a masternode.");
  await page.getByTestId("mn-summary-activity").click();
  await expect(page.getByTestId("tx-route-filter")).toContainText("Coinbase income");
  const rows = page.getByTestId("tx-list").locator("[data-txid]");
  await expect(rows.first()).toBeVisible();
  const shown = await rows.evaluateAll((els) => els.map((e) => e.getAttribute("data-txid")!));
  const types = await page.evaluate(() => Object.fromEntries(window.__dwFixture!.world.wallets[0].rows.map((r) => [r.txid, r.txType])));
  expect(shown.every((t) => types[t] === "generated")).toBe(true);
  expect(Object.values(types).some((t) => t !== "generated")).toBe(true);
  // Leaving Activity drops the filter.
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("sidebar-activity").click();
  await expect(page.getByTestId("tx-route-filter")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("locked → unlock re-reads the pane", async ({ page }) => {
  const errors = await open(page, "mn-locked");
  await row(page, CLUB).click();
  await expect(page.getByTestId("mn-registrar-line")).toContainText("Unlock to check the keys you added.");
  await page.getByTestId("mn-unlock").click();
  await page.getByTestId("unlock-sheet-field").fill("demo");
  await page.getByTestId("unlock-sheet-submit").click();
  await expect(page.getByTestId("unlock-sheet")).toHaveCount(0);
  await expect(page.getByTestId("mn-registrar-line")).toHaveText("This wallet doesn't hold this masternode's owner key.");
  expect(errors).toEqual([]);
});

test("payouts: a failed first read and a failed re-read show the error, never old totals", async ({ page }) => {
  const errors = await open(page, "mn-v24-payouts", "light", "&fail=mn_payouts:none");
  await row(page, HOME).click();
  await expect(page.getByTestId("mn-payouts-failed")).toContainText("Couldn't read this masternode's payouts.");
  await expect(page.getByTestId("mn-payouts")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.fail("mn_payouts", null));
  await page.getByTestId("mn-payouts-retry").click();
  await expect(page.getByTestId("mn-payouts-flagged")).toBeVisible();
  await expect(page.getByTestId("mn-payouts-failed")).toHaveCount(0);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.fail("mn_payouts", "none");
    f.emit({ kind: "historyChanged", network: "testnet", walletId: f.world.wallets[0].summary.walletId, txids: [] });
  });
  await expect(page.getByTestId("mn-payouts-failed")).toBeVisible();
  for (const gone of ["mn-payouts", "mn-payouts-flagged", "mn-flagged-activity", "mn-payouts-activity"]) await expect(page.getByTestId(gone)).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a failed pane re-read drops the detail facts and the unlock line", async ({ page }) => {
  await open(page, "mn-locked");
  await row(page, CLUB).click();
  await expect(page.getByTestId("mn-unlock")).toBeVisible();
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.fail("mn_masternode", "none");
    f.emit({ kind: "vaultLockState", network: "testnet", state: "unlockedMixingOnly" });
  });
  await expect(page.getByTestId("mn-detail-failed")).toBeVisible();
  await expect(page.getByTestId("mn-registrar-line")).toHaveCount(0);
  await expect(page.getByTestId("mn-unlock")).toHaveCount(0);
});

// The list height line comes from its own read, which the page starts when it opens. Until it
// answers, its line keeps its place, so nothing below moves under a click when it lands.
for (const [name, scenario, extra] of [
  ["answers", "mn-list", ""],
  ["fails", "mn-list", "&fail=mn_list_state:none"],
  ["says the list is syncing", "mn-syncing", ""],
] as const)
  test(`nothing moves when the list height read ${name}`, async ({ page }) => {
    const errors = await open(page, scenario, "light", `&hold=mn_list_state${extra}`);
    await row(page, HOME).click();
    const more = page.getByTestId("mn-more");
    await more.scrollIntoViewIfNeeded();
    const before = await more.boundingBox();
    await page.evaluate(() => window.__dwFixture!.release("mn_list_state"));
    await expect(page.getByTestId("mn-list-height")).toHaveCount(name === "answers" ? 1 : 0);
    // A failed or unsynced answer shows nothing to wait for: give it time to land.
    await page.waitForTimeout(300);
    expect(await more.boundingBox()).toEqual(before);
    await page.mouse.click(before!.x + before!.width / 2, before!.y + before!.height / 2);
    await expect(page.getByTestId("mn-more-rename")).toBeVisible();
    expect(errors).toEqual([]);
  });

test("states: empty, collateral coin, syncing, failed, watch-only", async ({ page }) => {
  await open(page, "mn-empty");
  await expect(page.getByTestId("mn-empty")).toContainText("No masternodes yet");
  await expect(page.getByTestId("mn-empty-coin")).toHaveCount(0);
  await open(page, "mn-empty-coin");
  await expect(page.getByTestId("mn-empty-coin")).toHaveText("This wallet holds a 1,000 Dash output that can be a masternode collateral.");
  await open(page, "mn-syncing");
  await expect(page.getByTestId("mn-syncing")).toContainText("Loading the masternode list…");
  await expect(page.getByTestId("mn-list-height")).toHaveCount(0);
  await expect(row(page, HOME)).toContainText("list syncing");
  await open(page, "mn-list-failed");
  await expect(page.getByTestId("mn-list-failed")).toContainText("Couldn't read your masternodes.");
  await expect(page.getByTestId("mn-retry")).toBeVisible();
  await open(page, "mn-watchonly");
  await expect(page.getByTestId("mn-notice-watch-only")).toContainText("Watch-only wallet.");
  await row(page, HOME).click();
  await expect(page.getByTestId("mn-roles")).toContainText("Owner key (watched)");
  await expect(page.getByTestId("mn-vote")).toBeDisabled();
  await expect(pane(page)).toContainText("Voting needs the wallet that holds the voting keys");
});

test("the open tab is kept in the settings", async ({ page }) => {
  await open(page, "mn-list");
  await page.getByTestId("masternodes-tabs-names").click();
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("masternodes-tabs-names")).toHaveAttribute("aria-selected", "true");
});
