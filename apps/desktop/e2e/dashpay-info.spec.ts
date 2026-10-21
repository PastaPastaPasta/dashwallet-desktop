import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP6-03: Tools ▸ Information's DashPay card (DASHPAY F23) on the fixture backend: each state from
// its scenario, Sync now (`dashpay_sync_soon`, D3), a failed read, no wallet, and axe in light and
// dark. Test names lead with the design note's D-row or the parity row they cover.

async function card(page: Page, scenario: string, theme: Theme = "light", extra = "", clock: "fixed" | "installed" = "fixed") {
  const errors = trackPageErrors(page);
  if (clock === "fixed") await page.clock.setFixedTime(FIXTURE_NOW);
  else await page.clock.install({ time: FIXTURE_NOW });
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "information" }));
  const c = page.getByTestId("information-dashpay");
  await c.waitFor();
  return { c, errors };
}

const state = (page: Page) => page.getByTestId("information-dashpay-state");
const calls = (page: Page, cmd: string) => page.evaluate((c) => window.__dwFixture!.calls.filter((x) => x === c).length, cmd);

/** Each state's scenario and headline (design note §2). */
const STATES: [scenario: string, state: string, headline: string][] = [
  ["dashpay-info-spv", "running", "Quorum keys verified by SPV"],
  ["dashpay-registered", "degraded", "Quorum keys from a trusted service"],
  ["dashpay-info-fallback", "degraded", "Trusted quorum keys until masternode sync finishes"],
  ["dashpay-info-trusted-only", "degraded", "SPV quorum checks are off (developer setting)"],
  ["dashpay-look-failed", "failed", "Could not finish looking for your DashPay identity"],
  ["dashpay-info-pass-failed", "failed", "The last DashPay pass failed"],
  ["dashpay-starting", "starting", "Looking for your DashPay identity…"],
  ["dashpay-not-started", "notStarted", "DashPay has not started"],
  ["dashpay-no-identity", "notSetUp", "No DashPay identity on this wallet"],
  ["dashpay-info-no-identity-proven", "notSetUp", "No DashPay identity on this wallet"],
  ["dashpay-info-stopped", "stopped", "DashPay sync is stopped"],
  ["dashpay-watch-only", "watchOnly", "DashPay needs this wallet's keys"],
  ["no-wallet", "noWallet", "No wallet on this network yet"],
];

for (const [scenario, name, headline] of STATES) {
  test(`D2: ${scenario} shows ${name}`, async ({ page }) => {
    const { errors } = await card(page, scenario);
    await expect(state(page)).toHaveAttribute("data-state", name);
    await expect(state(page)).toContainText(headline);
    expect(errors).toEqual([]);
  });
}

test("F23: Running lists the scan verdict, the last pass, the queue, the loops and the quorum keys", async ({ page }) => {
  const { c, errors } = await card(page, "dashpay-info-spv");
  await expect(state(page)).toHaveAttribute("data-state", "running");
  const rows = page.getByTestId("information-dashpay-rows");
  await expect(rows).toContainText("Identity scan");
  await expect(rows).toContainText("Complete: identity found");
  await expect(rows).toContainText("2 new requests, 1 new contact");
  await expect(rows).toContainText("Contacts waiting for an unlock");
  await expect(rows.locator(".detail-row", { hasText: "Contacts waiting for an unlock" })).toContainText("2");
  await expect(rows.locator(".detail-row", { hasText: "Contacts and payments loop" })).toContainText("Running · last run 11:59:55 · next 12:00:10");
  await expect(rows).toContainText("SPV (verified masternode list)");
  await expect(c.getByTestId("information-dashpay-sync-now")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("D3: Sync now asks the engine for a pass, says so, and reads the card again 5 s later", async ({ page }) => {
  const { c } = await card(page, "dashpay-info-spv", "light", "", "installed");
  await expect(state(page)).toHaveAttribute("data-state", "running");
  // The mount's reads have landed: the count holds still.
  await page.clock.runFor(1_000);
  const before = await calls(page, "dashpay_sync_status");
  await c.getByTestId("information-dashpay-sync-now").click();
  await expect(page.getByTestId("information-dashpay-outcome")).toHaveText("A DashPay pass was asked for. This card shows it once it reports.");
  expect(await calls(page, "dashpay_sync_soon")).toBe(1);
  await page.clock.runFor(4_900);
  expect(await calls(page, "dashpay_sync_status")).toBe(before);
  await page.clock.runFor(200);
  await expect.poll(() => calls(page, "dashpay_sync_status")).toBe(before + 1);
});

test("D3: a refused Sync now says why", async ({ page }) => {
  const { c } = await card(page, "dashpay-info-spv", "light", "&fail=dashpay_sync_soon:none");
  await c.getByTestId("information-dashpay-sync-now").click();
  await expect(page.getByTestId("information-dashpay-outcome")).toHaveAttribute("role", "alert");
  await expect(page.getByTestId("information-dashpay-outcome")).toContainText("Could not ask for a DashPay pass");
});

test("D3: with the loops stopped, the card says Stopped and Sync now is held with the reason", async ({ page }) => {
  const { c } = await card(page, "dashpay-info-stopped");
  await expect(state(page)).toHaveAttribute("data-state", "stopped");
  const button = c.getByTestId("information-dashpay-sync-now");
  await expect(button).toBeDisabled();
  const note = page.getByTestId("information-dashpay-sync-note");
  await expect(note).toContainText("DashPay's sync loops are not running.");
  await expect(button).toHaveAttribute("aria-describedby", (await note.getAttribute("id"))!);
  await expect(page.getByTestId("information-dashpay-rows")).toContainText("Stopped · last run");
});

test("D2: a watch-only wallet's Sync now is held with the reason", async ({ page }) => {
  const { c } = await card(page, "dashpay-watch-only");
  await expect(c.getByTestId("information-dashpay-sync-now")).toBeDisabled();
  await expect(page.getByTestId("information-dashpay-sync-note")).toHaveText("A watch-only wallet can't use DashPay because it has no keys.");
});

test("D7: a failed read says so, and Try again reads it again", async ({ page }) => {
  const { c } = await card(page, "dashpay-info-spv", "light", "&fail=dashpay_sync_status:none");
  await expect(state(page)).toHaveAttribute("data-state", "error");
  await expect(c.getByTestId("information-dashpay-sync-now")).toHaveCount(0);
  await expect(page.getByTestId("information-dashpay-rows")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_sync_status", null));
  await c.getByTestId("information-dashpay-retry").click();
  await expect(state(page)).toHaveAttribute("data-state", "running");
});

test("D7: a failed wallet list says so, and Try again reads it again", async ({ page }) => {
  const { c } = await card(page, "no-wallet", "light", "&fail=list_wallets:none");
  await expect(state(page)).toHaveAttribute("data-state", "error");
  await page.evaluate(() => window.__dwFixture!.fail("list_wallets", null));
  await c.getByTestId("information-dashpay-retry").click();
  await expect(state(page)).toHaveAttribute("data-state", "noWallet");
});

test("D7: a failed wallet-list refetch keeps the card, which was read", async ({ page }) => {
  await card(page, "dashpay-info-spv");
  await expect(state(page)).toHaveAttribute("data-state", "running");
  const before = await calls(page, "list_wallets");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.fail("list_wallets", "none");
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, balances: null });
  });
  await expect.poll(() => calls(page, "list_wallets")).toBeGreaterThan(before);
  // The call is counted before it throws: let the failure land and render first.
  await page.evaluate(() => new Promise((r) => setTimeout(() => requestAnimationFrame(() => requestAnimationFrame(r)), 50)));
  await expect(state(page)).toHaveAttribute("data-state", "running");
});

test("D6: showing the card again reads it again", async ({ page }) => {
  await card(page, "dashpay-info-spv");
  await expect(state(page)).toHaveAttribute("data-state", "running");
  const before = await calls(page, "dashpay_sync_status");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "peers" }));
  await expect(page.getByTestId("information-dashpay")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "information" }));
  await expect.poll(() => calls(page, "dashpay_sync_status")).toBeGreaterThan(before);
});

test("D6: SPV stopping re-reads the card", async ({ page }) => {
  await card(page, "dashpay-info-spv");
  await expect(state(page)).toHaveAttribute("data-state", "running");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.sync = { ...f.world.sync, running: false };
    f.emit({ kind: "spvStateChanged", network: f.world.launch.network.dirName, running: false });
  });
  await expect(state(page)).toHaveAttribute("data-state", "stopped");
});

test("D6: a Platform signal re-reads the card", async ({ page }) => {
  await card(page, "dashpay-registered");
  await expect(state(page)).toHaveAttribute("data-state", "degraded");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.trust = { source: "spv", trustedOnly: false };
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } });
  });
  await expect(state(page)).toHaveAttribute("data-state", "running");
});

test("D2: no wallet shows the session's quorum source", async ({ page }) => {
  const { c } = await card(page, "no-wallet");
  await expect(page.getByTestId("information-dashpay-rows")).toHaveText(/Quorum keys\s*Trusted service \(degraded mode\)/);
  await expect(c.getByTestId("information-dashpay-sync-now")).toHaveCount(0);
});

for (const theme of THEMES) {
  for (const scenario of STATES.map(([s]) => s)) {
    test(`axe: the DashPay card, ${scenario}, ${theme}`, async ({ page }) => {
      const { c } = await card(page, scenario, theme);
      await expect(state(page)).not.toHaveAttribute("data-state", "loading");
      await c.scrollIntoViewIfNeeded();
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
    });
  }
}
