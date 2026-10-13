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
 * screen shows a reconciliation that started after the change (`data-checked-at`). */
async function settleAndReconcile(page: Page, change: "artifacts" | "leases") {
  const changedAt = await page.evaluate((change) => {
    const f = window.__dwFixture!;
    const walletId = f.world.wallets[0].summary.walletId;
    if (change === "artifacts") for (const artifact of Object.keys(f.world.dispatch)) f.world.dispatch[artifact] = "sent";
    else f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "ended" }, inFlight: 0, callRunning: false }));
    const changedAt = Date.now();
    for (const lease of f.world.leases) f.emit({ kind: "leaseChanged", network: "testnet", lease });
    const artifact = f.world.cut.flows[0].artifacts[0].artifact;
    f.emit({ kind: "dispatchResolved", network: "testnet", settled: { walletId, artifact, resolution: "sent" } });
    return changedAt;
  }, change);
  await expect
    .poll(async () => Number(await page.getByTestId("lock-report").getAttribute("data-checked-at")))
    .toBeGreaterThan(changedAt);
}

test("flows omitted after pending ones keep the report unresolved whatever settles", async ({ page }) => {
  await lockFrom(page, "lock-report-omitted");
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(notice.getByRole("status")).toHaveText("Lock stops new signatures; a transaction already signed may still be sent");
  await expect(page.getByTestId("lock-report-flow")).toHaveCount(16);
  // Listed unresolved first by outcome: the registration reads Sent (its funding was), so it is last.
  await expect(page.getByTestId("lock-report-flow").first()).toHaveText("Credit top-up: Will be sent");
  await expect(page.getByTestId("lock-report-flow").last()).toHaveText("Username registration: Sent");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText("3 more actions, 7 more steps, 2 more wallets not listed here.");
  const beyondReach = "The app cannot check all of these actions, so some may still be sent.";
  await expect(page.getByTestId("lock-report-check")).toHaveText(beyondReach);

  await settleAndReconcile(page, "artifacts");
  await settleAndReconcile(page, "leases");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-check")).toHaveText(beyondReach);
});

test("omissions the order shows are settled resolve with the listed artifacts", async ({ page }) => {
  await lockFrom(page, "lock-report-omitted-settles");
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText("2 more actions, 4 more steps, 2 more wallets not listed here.");
  await expect(page.getByTestId("lock-report-check")).toHaveText("Checking whether any of these actions may still be sent…");

  // The listed artifacts settle, but the registration's call is still running.
  await settleAndReconcile(page, "artifacts");
  await expect(notice).toHaveAttribute("data-resolved", "false");

  await settleAndReconcile(page, "leases");
  await expect(notice).toHaveAttribute("data-resolved", "true");
  await expect(notice.getByRole("status")).toHaveText("Every action the lock stopped has ended.");
  await expect(page.getByTestId("lock-report-check")).toHaveCount(0);
  await expect(page.getByTestId("lock-report-omitted")).toHaveText("2 more actions, 4 more steps, 2 more wallets not listed here.");
});

test("a sent flow with a step still out is unresolved until the step settles", async ({ page }) => {
  await lockFrom(page, "lock-report-pending");
  const notice = page.getByTestId("lock-report");
  // E0-04 §8.4: the registration's funding was sent, so it reads Sent, but its identity call may
  // have gone out.
  await expect(page.getByTestId("lock-report-flow").filter({ hasText: "Username registration" })).toHaveText("Username registration: Sent");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-check")).toHaveText("Checking whether any of these actions may still be sent…");

  await settleAndReconcile(page, "artifacts");
  await expect(notice).toHaveAttribute("data-resolved", "true");
});

test("a call reported under its label cannot be checked, so it stays unresolved", async ({ page }) => {
  await lockFrom(page, "lock-report-call");
  const notice = page.getByTestId("lock-report");
  await expect(page.getByTestId("lock-report-flow")).toHaveText(["Accept and pay: Sent"]);
  await expect(page.getByTestId("lock-report-check")).toHaveText("The app cannot check all of these actions, so some may still be sent.");
  await settleAndReconcile(page, "leases");
  await expect(notice).toHaveAttribute("data-resolved", "false");
});

test("a failed check says so, complete report or not", async ({ page }) => {
  await page.goto(fixtureUrl("lock-report-pending", "light", "&fail=leases:none"));
  await page.getByTestId("toolbar-lock").click();
  await expect(page.getByTestId("lock-report")).toHaveAttribute("data-resolved", "false");
  await expect(page.getByTestId("lock-report-check")).toHaveText("Could not check whether any of these actions may still be sent.");
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

test("a slow report reply cannot bring back the previous lock's report", async ({ page }) => {
  await lockFrom(page, "lock-report-resolved");
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "true");

  // A refetch of every query (an overflow) whose lastLockReport reply, read now, arrives late.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    (window as unknown as { release: () => void }).release = f.hold("last_lock_report");
    f.emit({ kind: "eventsOverflowed" });
  });
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("last_lock_report"))).toBe(true);

  // Meanwhile a newer lock ends with a report that leaves flows out.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const newer = { ...f.world.lastLockReport!, flowsOmitted: 3 };
    f.world.lastLockReport = newer;
    f.emit({ kind: "lockProgress", network: "testnet", progress: { kind: "done", report: newer } });
  });
  const omitted = page.getByTestId("lock-report-omitted");
  await expect(omitted).toHaveText("3 more actions not listed here.");

  // The late reply lands: the screen keeps the newer report.
  await page.evaluate(async () => {
    (window as unknown as { release: () => void }).release();
    await new Promise((r) => setTimeout(r, 250));
  });
  expect(await omitted.textContent()).toBe("3 more actions not listed here.");
});

test("a lock that cut nothing shows no report", async ({ page }) => {
  await lockFrom(page, "funded");
  await page.getByTestId("lock-screen").waitFor();
  await expect(page.getByTestId("lock-report")).toHaveCount(0);
});
