import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl } from "./helpers";

// E0-13 D3 (Sol r4 Low 1): a lock report with `flowsOmitted`, `artifactsOmitted` or
// `walletsWithSecretsOmitted` above 0 is unresolved. The lock screen says so until `leases`,
// `dispatchStatus` and `lockStatus` show nothing can still be sent, and for good when what the
// report left out may be pending (it has no ids to ask about).

async function lockFrom(page: Page, scenario: string) {
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar-lock").click();
}

/** Changes the fixture's state and announces it as the engine would, then waits until the lock
 * screen has re-run its reconciliation (a `dispatch_status` call after the events). */
async function settleAndReconcile(page: Page, change: string) {
  const before = await page.evaluate(() => window.__dwFixture!.calls.length);
  await page.evaluate((change) => {
    const f = window.__dwFixture!;
    const walletId = f.world.wallets[0].summary.walletId;
    if (change === "artifacts") for (const artifact of Object.keys(f.world.dispatch)) f.world.dispatch[artifact] = "sent";
    else f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "ended" }, inFlight: 0, callRunning: false }));
    for (const lease of f.world.leases) f.emit({ kind: "leaseChanged", network: "testnet", lease });
    f.emit({ kind: "dispatchResolved", network: "testnet", settled: { walletId, artifact: "tx:3f9a1c27", resolution: "sent" } });
  }, change);
  await expect.poll(() => page.evaluate((n) => window.__dwFixture!.calls.slice(n).includes("dispatch_status"), before)).toBe(true);
}

test("flows omitted after pending ones keep the report unresolved whatever settles", async ({ page }) => {
  await lockFrom(page, "lock-report-omitted");
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(notice.getByRole("status")).toHaveText("Lock stops new signatures; a transaction already signed may still be sent");
  await expect(page.getByTestId("lock-report-flow")).toHaveCount(16);
  await expect(page.getByTestId("lock-report-flow").first()).toHaveText("Username registration: May have been sent");
  const omitted = "3 more actions, 7 more steps, 2 more wallets not listed here. The app cannot check them, so some may still be sent.";
  await expect(page.getByTestId("lock-report-omitted")).toHaveText(omitted);

  await settleAndReconcile(page, "artifacts");
  await settleAndReconcile(page, "leases");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText(omitted);
});

test("omissions the order shows are settled resolve with the listed artifacts", async ({ page }) => {
  await lockFrom(page, "lock-report-omitted-settles");
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText(
    "2 more actions, 4 more steps, 2 more wallets not listed here. Checking whether they may still be sent…",
  );

  // The listed artifacts settle, but the registration's call is still running.
  await settleAndReconcile(page, "artifacts");
  await expect(notice).toHaveAttribute("data-resolved", "false");

  await settleAndReconcile(page, "leases");
  await expect(notice).toHaveAttribute("data-resolved", "true");
  await expect(notice.getByRole("status")).toHaveText("Every action the lock stopped has ended.");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText("2 more actions, 4 more steps, 2 more wallets not listed here.");
});

test("a complete report of ended flows is resolved at once", async ({ page }) => {
  await lockFrom(page, "lock-report-resolved");
  await expect(page.getByTestId("lock-report")).toHaveAttribute("data-resolved", "true");
  await expect(page.getByTestId("lock-report-omitted")).toHaveCount(0);
});

test("while a newer lock drains, the previous report is not shown", async ({ page }) => {
  await lockFrom(page, "lock-report-resolved");
  await expect(page.getByTestId("lock-report")).toHaveAttribute("data-resolved", "true");
  await page.evaluate(() => window.__dwFixture!.emit({ kind: "lockProgress", network: "testnet", progress: { kind: "draining", inFlight: 2, deadlineInMs: 30_000 } }));
  await expect(page.getByTestId("lock-drain").getByRole("status")).toHaveText("Stopping 2 running actions…");
  await expect(page.getByTestId("lock-report")).toHaveCount(0);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.emit({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report: f.world.lastLockReport! } });
  });
  await expect(page.getByTestId("lock-drain")).toHaveCount(0);
  await expect(page.getByTestId("lock-report")).toHaveAttribute("data-resolved", "true");
});

test("a lock that cut nothing shows no report", async ({ page }) => {
  await lockFrom(page, "funded");
  await page.getByTestId("lock-screen").waitFor();
  await expect(page.getByTestId("lock-report")).toHaveCount(0);
});
