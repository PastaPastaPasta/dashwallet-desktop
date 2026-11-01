import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl } from "./helpers";

// DEC-1058 fix round 1 (Sol's review): the cases the first batch's tests did not reach. Each test
// here failed on d74b2dab.

/** The toast region's text right now: no retry. */
const toastText = (page: Page) => page.locator(".toast-host").evaluate((e) => e.textContent ?? "");

const pay = (page: Page, duffs: number) => page.evaluate(`window.__dwFixture.receivePayment(${duffs})`) as Promise<string>;

async function open(page: Page, scenario = "funded") {
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero").waitFor();
}

test.describe("payment toast privacy", () => {
  test("a toast already on screen is masked when Discreet turns on", async ({ page }) => {
    await open(page);
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received a payment");
    expect(await toastText(page)).not.toContain("0.00999819");
    // And back: the toast is the same one, not a new announcement.
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
  });

  test("a toast on screen is dismissed when the wallet locks", async ({ page }) => {
    await open(page);
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    await page.getByTestId("toolbar-lock").click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    // Read now, not retried: a retrying assertion would pass once the toast timed out by itself.
    expect(await toastText(page)).toBe("");
    // A payment that arrives while locked is not announced either.
    await pay(page, 150_000_000);
    await page.waitForTimeout(400);
    expect(await toastText(page)).toBe("");
    await authorizeUnlock(page);
  });

  test("a payment that arrives in Discreet mode is masked from the start", async ({ page }) => {
    await open(page);
    await page.getByTestId("toolbar-discreet").click();
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received a payment");
  });
});

/** An engine `sync` event that says `behind` blocks short of the tip (0: caught up). */
const syncEvent = (page: Page, behind: number, caughtUp = behind === 0) =>
  page.evaluate(`(() => {
    const f = window.__dwFixture;
    const s = f.world.sync;
    const tip = s.tipHeight;
    f.world.sync = { ...s, caughtUp: ${caughtUp}, activePhase: ${caughtUp} ? null : "filters", phases: s.phases.map((p) => ({ ...p, currentHeight: tip - ${behind}, targetHeight: tip, done: ${behind} === 0 })) };
    f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
  })()`);

test.describe("sync hold", () => {
  test("Synced does not outlive 60 s with no further engine signal", async ({ page }) => {
    await page.clock.install();
    await open(page);
    const bar = page.getByTestId("status-bar");
    await expect(bar).toContainText("Synced");
    // One block behind: held at Synced.
    await syncEvent(page, 1);
    await page.clock.runFor(2000);
    await expect(bar).toContainText("Synced");
    // Nothing else happens for 61 s: the hold expires on its own.
    await page.clock.runFor(61_000);
    await expect(bar).toContainText("Syncing");
    await expect(bar).not.toContainText("Synced");
  });
});

test.describe("catch-up is not news", () => {
  /** An incoming row of the fixture's history, not yet announced. */
  const unseen = (page: Page) =>
    page.evaluate(`(() => { const w = window.__dwFixture.world.wallets[0]; const r = w.rows.find((x) => x.txType === "recvWithAddress" && x.amount > 0); return r.txid; })()`) as Promise<string>;
  const catchUp = (page: Page, txid: string) =>
    page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "newTransactions", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, txids: [${JSON.stringify(txid)}], count: 1, catchUp: true }); })()`);
  const noticeCalls = (page: Page) => page.evaluate(`window.__dwFixture.calls.filter((c) => c === "tx_notices").length`) as Promise<number>;

  test("a far-behind sync event, its read still pending, and a catch-up batch announce nothing", async ({ page }) => {
    await open(page);
    const txid = await unseen(page);
    // The sync read the event triggers is held: the display cache still says synced.
    await page.evaluate(`window.__dwFixture.hold("sync_status")`);
    await syncEvent(page, 5000);
    await catchUp(page, txid);
    await page.waitForTimeout(500);
    expect(await noticeCalls(page)).toBe(0);
    await expect(page.locator(".toast")).toHaveCount(0);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
  });

  test("a catch-up batch while caught up is a new block: announced", async ({ page }) => {
    await open(page);
    const txid = await unseen(page);
    await catchUp(page, txid);
    await expect(page.locator(".toast")).toHaveText("Received 0.20 tDASH");
    expect(await noticeCalls(page)).toBe(1);
  });
});

async function authorizeUnlock(page: Page) {
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await page.getByTestId("hero").waitFor();
}

void authorize;
