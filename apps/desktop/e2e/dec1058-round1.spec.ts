import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl } from "./helpers";
import { PAYEE } from "./wallet";

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

test.describe("read marking", () => {
  test("the popover marks read only the rows it shows", async ({ page }) => {
    await open(page, "dashpay-registered");
    for (let i = 1; i <= 6; i++) await pay(page, i * 1_000_000);
    // The identity's three unread events and six payments.
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("9");
    await page.getByTestId("toolbar-bell").click();
    await expect(page.getByTestId("bell-payment")).toHaveCount(5);
    // The sixth was not shown: it is still unread (the events were shown).
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("1");
    await page.getByTestId("bell-see-all").click();
    await expect(page.getByTestId("bell-payment")).toHaveCount(6);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
  });

  test("a Notifications page behind the lock screen marks nothing read", async ({ page }) => {
    await open(page);
    await page.getByTestId("toolbar-bell").click();
    await expect(page.getByTestId("notifications-payments")).toBeAttached();
    await page.getByTestId("toolbar-lock").click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    await pay(page, 150_000_000);
    await page.waitForTimeout(500);
    // The badge is in the (inert) toolbar: still unread, not cleared behind the lock screen.
    expect(await page.getByTestId("toolbar-bell-badge").evaluate((e) => e.textContent)).toBe("1");
    await authorizeUnlock(page, "notifications-payments");
    // Unlocked, the page shows it: now read.
    await expect(page.getByTestId("bell-payment")).toHaveCount(1);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
  });
});

test.describe("Receive waits for the notice lookup", () => {
  test("a tx_notices answer slower than the grace still shows Received before the next address", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-receive").click();
    const address = page.getByTestId("receive-address");
    const first = (await address.textContent())!;
    const release = await page.evaluateHandle(`window.__dwFixture.hold("tx_notices")`);
    await pay(page, 999_819);
    // Well past the 1.5 s grace: the lookup is outstanding, so the address has not moved.
    await page.waitForTimeout(2500);
    await expect(address).toHaveText(first);
    await release.evaluate((f) => (f as unknown as () => void)());
    await expect(page.getByTestId("receive-received")).toHaveText("✓ Received 0.00999819 tDASH");
    await expect(address).toHaveText(first);
    await expect(address).not.toHaveText(first, { timeout: 8000 });
  });
});

test.describe("Pay's confirm sheet warns about fees", () => {
  async function review(page: Page, amount: string, subtract = false) {
    await open(page);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill(amount);
    if (subtract) await page.getByTestId("pay-subtract-fee").check();
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("pay-confirm")).toBeVisible();
  }

  test("22% of a small payment says so", async ({ page }) => {
    await review(page, "0.00001");
    await expect(page.getByTestId("pay-confirm-fee")).toContainText("0.00000226");
    await expect(page.getByTestId("pay-confirm-fee-warning")).toHaveText("Fees are 22% of this amount.");
  });

  test("a payment well above ten times the fee has no warning", async ({ page }) => {
    await review(page, "0.5");
    await expect(page.getByTestId("pay-confirm-fee")).toBeVisible();
    await expect(page.getByTestId("pay-confirm-fee-warning")).toHaveCount(0);
  });

  test("with the fee subtracted it counts against what the payee receives", async ({ page }) => {
    await review(page, "0.00001", true);
    // The payee gets 1000 - 226 duffs: the fee is 29% of that.
    await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.00000774");
    await expect(page.getByTestId("pay-confirm-fee-warning")).toHaveText("Fees are 29% of this amount.");
  });
});

async function authorizeUnlock(page: Page, back = "hero") {
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await page.getByTestId(back).waitFor();
}

void authorize;
