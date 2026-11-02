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
    // The page's clock is ours: a toast lives 3 s, and under load the steps below can take longer
    // than that of real time. Once it is on screen, time stops until the test lets it run.
    await page.clock.install();
    await open(page);
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    // From the page's own time, a little ahead: the jump takes as little of the toast's 3 s as it can.
    await page.clock.pauseAt(new Date((await page.evaluate(() => Date.now())) + 100));
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received a payment");
    expect(await toastText(page)).not.toContain("0.00999819");
    // And back: the toast is the same one, not a new announcement.
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    // It goes when its time is up, and not before.
    await page.clock.runFor(4000);
    await expect(page.locator(".toast")).toHaveCount(0);
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

  test("a payment of a block the engine took over a minute on is still announced, though the bar stopped holding", async ({ page }) => {
    await page.clock.install();
    await open(page);
    const txid = await unseen(page);
    const bar = page.getByTestId("status-bar");
    await syncEvent(page, 1);
    await page.clock.runFor(61_000);
    // The display no longer holds Synced for this block...
    await expect(bar).toContainText("Syncing");
    await catchUp(page, txid);
    await page.clock.runFor(500);
    // ...but the payment found in it is still news.
    expect(await noticeCalls(page)).toBe(1);
    await expect(page.locator(".toast")).toHaveText("Received 0.20 tDASH");
  });

  test("a payment found while far behind is history, however long since the last caught-up reading", async ({ page }) => {
    await page.clock.install();
    await open(page);
    const txid = await unseen(page);
    await syncEvent(page, 5000);
    await page.clock.runFor(61_000);
    await catchUp(page, txid);
    await page.clock.runFor(500);
    expect(await noticeCalls(page)).toBe(0);
  });

  test("a catch-up batch while caught up is a new block: announced", async ({ page }) => {
    await open(page);
    const txid = await unseen(page);
    await catchUp(page, txid);
    await expect(page.locator(".toast")).toHaveText("Received 0.20 tDASH");
    expect(await noticeCalls(page)).toBe(1);
  });
});

test.describe("round 3: the first read at startup", () => {
  const readsOfSync = (page: Page) => page.evaluate(`window.__dwFixture.calls.filter((c) => c === "sync_status").length`) as Promise<number>;

  test("an event that supersedes the first read does not leave Not syncing: a fresh read follows", async ({ page }) => {
    // The first sync_status call is held; its reply (running) is as of the call.
    await page.goto(fixtureUrl("funded", "light", "&hold=sync_status"));
    await page.getByTestId("status-bar").waitFor();
    // A queued event carries a snapshot that is already behind the engine's live state.
    await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: { ...f.world.sync, running: false, caughtUp: false } }); })()`);
    await page.evaluate(`window.__dwFixture.release("sync_status")`);
    // The engine is running: one more read, begun after the event, says so.
    await expect(page.getByTestId("status-bar")).toContainText("Synced");
    expect(await readsOfSync(page)).toBe(2);
  });
});

test.describe("round 2: stale sync replies", () => {
  const emitCurrent = (page: Page) => page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync }); })()`);
  const readsOfSync = (page: Page) => page.evaluate(`window.__dwFixture.calls.filter((c) => c === "sync_status").length`) as Promise<number>;

  test("an older caught-up reply released after a stall does not restore Synced, or catch-up news", async ({ page }) => {
    await page.clock.install();
    await open(page);
    const bar = page.getByTestId("status-bar");
    await expect(bar).toContainText("Synced");
    const txid = (await page.evaluate(
      `(() => { const w = window.__dwFixture.world.wallets[0]; return w.rows.find((x) => x.txType === "recvWithAddress" && x.amount > 0).txid; })()`,
    )) as string;
    const release = await page.evaluateHandle(`window.__dwFixture.hold("sync_status")`);
    // A read starts while the engine says caught up, and its reply is held (IPC is slow).
    const before = await readsOfSync(page);
    await emitCurrent(page);
    await expect.poll(() => readsOfSync(page)).toBeGreaterThan(before);
    // Then the engine says one block behind, and 45 s later says it again (a stall reading).
    await syncEvent(page, 1);
    await page.clock.runFor(45_000);
    await syncEvent(page, 1);
    // The old reply arrives.
    await release.evaluate((f) => (f as unknown as () => void)());
    await page.clock.runFor(120_000);
    await expect(bar).toContainText("Syncing");
    await expect(bar).not.toContainText("Synced");
    // The old reply did not restore a caught-up reading: once the notification allowance (10 minutes
    // since the last one) has passed, a catch-up batch is history, not news.
    await page.clock.runFor(10 * 60_000);
    await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "newTransactions", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, txids: [${JSON.stringify(txid)}], count: 1, catchUp: true }); })()`);
    await page.clock.runFor(500);
    expect(await page.evaluate(`window.__dwFixture.calls.filter((c) => c === "tx_notices").length`)).toBe(0);
    expect(await toastText(page)).toBe("");
  });
});

test.describe("round 2: catch-up near the tip", () => {
  test("a wallet that starts one block behind does not announce history", async ({ page }) => {
    await open(page, "sync-one-behind");
    await expect(page.getByTestId("status-bar")).toContainText("Syncing");
    const txid = (await page.evaluate(
      `(() => { const w = window.__dwFixture.world.wallets[0]; return w.rows.find((x) => x.txType === "recvWithAddress" && x.amount > 0).txid; })()`,
    )) as string;
    await page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "newTransactions", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, txids: [${JSON.stringify(txid)}], count: 1, catchUp: true }); })()`);
    await page.waitForTimeout(500);
    expect(await page.evaluate(`window.__dwFixture.calls.filter((c) => c === "tx_notices").length`)).toBe(0);
    expect(await toastText(page)).toBe("");
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
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

  test("a payment that arrives behind a transaction's details stays unread until they close", async ({ page }) => {
    await open(page);
    await pay(page, 150_000_000);
    await page.getByTestId("toolbar-bell").click();
    await expect(page.getByTestId("bell-payment")).toHaveCount(1);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
    await page.getByTestId("bell-payment").locator(".bell-row-open").click();
    await expect(page.locator(".sheet")).toBeVisible();
    await pay(page, 250_000_000);
    await page.waitForTimeout(500);
    // The badge is in the inert toolbar: read now, not retried.
    expect(await page.getByTestId("toolbar-bell-badge").evaluate((e) => e.textContent)).toBe("1");
    await page.keyboard.press("Escape");
    await expect(page.locator(".sheet")).toHaveCount(0);
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

  test("a Send in progress says Sending… on the button only, with no second line", async ({ page }) => {
    await review(page, "0.5");
    await page.evaluate(`window.__dwFixture.hold("draft_prepare")`);
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-confirm-send")).toHaveText("Sending…");
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    expect(await page.getByTestId("pay-confirm").locator(".dash-button-why").count()).toBe(0);
  });

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

test.describe("disabled reasons are visible and described", () => {
  /** The visible text a disabled control is described by. */
  async function reasonOf(page: Page, testId: string) {
    const button = page.getByTestId(testId);
    await expect(button).toBeDisabled();
    const id = await button.getAttribute("aria-describedby");
    expect(id, `${testId} is described`).toBeTruthy();
    const text = page.locator(`[id="${id}"]`);
    await expect(text).toBeVisible();
    return (await text.textContent())!.trim();
  }

  test("the Shielded card's actions, Finish included, are described by the syncing text", async ({ page }) => {
    await page.goto(fixtureUrl("shielded-syncing-pending", "light"));
    await page.getByTestId("shielded-card").waitFor();
    for (const id of ["shielded-receive-open", "shielded-send", "shielded-add", "shielded-out", "shielded-finish"]) {
      expect(await reasonOf(page, id)).toContain("Shielded is still syncing; try again when it finishes.");
    }
  });

  test("the Shielded card says why Send and Move wait with nothing in Shielded", async ({ page }) => {
    await page.goto(fixtureUrl("shielded-empty", "light"));
    await page.getByTestId("shielded-card").waitFor();
    for (const id of ["shielded-send", "shielded-out"]) expect(await reasonOf(page, id)).toContain("Nothing in Shielded to send yet.");
    await expect(page.getByTestId("shielded-add")).toBeEnabled();
  });

  test("Shielded's Next says what is missing, and waits while Shielded syncs", async ({ page }) => {
    await open(page);
    await page.getByTestId("shielded-add").click();
    expect(await reasonOf(page, "shielded-next")).toBe("Enter an amount first.");
    await page.getByTestId("shielded-amount").fill("1");
    await expect(page.getByTestId("shielded-next")).toBeEnabled();
    // Readiness lost with the form open: Next waits too.
    await page.evaluate(`(() => { const f = window.__dwFixture; f.world.pending.shielded.ready = false; f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "startup" } }); f.emit({ kind: "eventsOverflowed" }); })()`);
    // The form stays open; Confirm and Next both wait.
    expect(await reasonOf(page, "shielded-next")).toContain("Shielded is still syncing; try again when it finishes.");
  });

  test("an inactive Shielded card has no reason lines for actions it does not show", async ({ page }) => {
    await page.goto(fixtureUrl("shielded-inactive", "light"));
    await page.getByTestId("shielded-card").waitFor();
    await expect(page.getByTestId("shielded-send")).toHaveCount(0);
    await expect(page.getByTestId("shielded-card-why")).toHaveCount(0);
  });

  test("Pay keeps the fee line while it re-estimates, and does not ask for what is entered", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.evaluate(`window.__dwFixture.hold("draft_estimate")`);
    await page.getByTestId("pay-amount").fill("0.5");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
    await expect(page.getByTestId("pay-incomplete")).toHaveText("Checking the payment…");
  });

  test("Activity's Export CSV reason sits under the header, not inside its action row", async ({ page }) => {
    await page.goto(fixtureUrl("empty", "light"));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-export-why")).toBeVisible();
    await expect(page.locator('.tx-intro-actions [data-testid="tx-export-why"]')).toHaveCount(0);
    await expect(page.locator('.tx-intro-actions .dash-button-why')).toHaveCount(0);
  });

  test("Export CSV and Pay's Review say why, beside the control", async ({ page }) => {
    await page.goto(fixtureUrl("empty", "light"));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("sidebar-activity").click();
    expect(await reasonOf(page, "tx-export-csv")).toBe("There are no transactions to export yet.");
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    expect(await reasonOf(page, "pay-continue")).toBe("Enter an address and an amount first.");
  });
});

test("the PSBT's copy button answers like every other copy button", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) } });
  });
  await open(page);
  await page.evaluate(`document.activeElement?.blur(); window.__dwFixture.shellCommand({ kind: "tools", tab: "psbt" })`);
  await expect(page.getByTestId("tools-tabs-psbt")).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("psbt-paste").fill(Buffer.from("psbt\xffunsigned", "latin1").toString("base64"));
  await page.getByTestId("psbt-load-text").click();
  const button = page.getByTestId("psbt-copy");
  await expect(button).toHaveText("Copy to Clipboard");
  await button.click();
  await expect(button).toHaveText("✓ Copied");
  await expect(page.locator(".toast")).toHaveText("Copied");
  await expect(page.getByTestId("psbt-message")).toHaveText("PSBT copied to clipboard.");
  await expect(button).toHaveText("Copy to Clipboard", { timeout: 8000 });
});

async function authorizeUnlock(page: Page, back = "hero") {
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await page.getByTestId(back).waitFor();
}

void authorize;
