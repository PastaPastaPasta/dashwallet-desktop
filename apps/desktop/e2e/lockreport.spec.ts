import { expect, test } from "@playwright/test";
import { fixtureUrl } from "./helpers";

// E0-13 D3 (Sol r4 Low 1): a lock report with `flowsOmitted`, `artifactsOmitted` or
// `walletsWithSecretsOmitted` above 0 is unresolved, and the lock screen says so until
// `leases` and `dispatchStatus` show nothing can still be sent.

test("a report with omitted counts stays unresolved after its listed flows settle", async ({ page }) => {
  await page.goto(fixtureUrl("lock-report-omitted", "light"));
  await page.getByTestId("toolbar-lock").click();
  const notice = page.getByTestId("lock-report");
  await expect(notice).toHaveAttribute("data-resolved", "false");
  await expect(notice.getByRole("status")).toHaveText("Lock stops new signatures; a transaction already signed may still be sent");
  await expect(page.getByTestId("lock-report-flow")).toHaveText([
    "Username registration: May have been sent",
    "Credit top-up: Will be sent",
    "Accept and pay: Sent",
    "Contact request: Cancelled",
  ]);
  await expect(page.getByTestId("lock-report-omitted")).toHaveText(
    "3 more actions, 7 more steps, 2 more wallets not listed here. Checking whether they may still be sent…",
  );

  // Both listed artifacts settle: still unresolved, because leases hold committed funds.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.dispatch["tx:3f9a1c27"] = "notSent";
    f.world.dispatch["st:8be04d11"] = "sent";
    f.emit({ kind: "dispatchResolved", network: "testnet", settled: { walletId: f.world.wallets[0].summary.walletId, artifact: "st:8be04d11", resolution: "sent" } });
  });
  await page.waitForTimeout(300);
  await expect(notice).toHaveAttribute("data-resolved", "false");

  // Every lease ends: nothing left that could send what the report left out.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "ended" }, inFlight: 0, callRunning: false }));
    for (const lease of f.world.leases) f.emit({ kind: "leaseChanged", network: "testnet", lease });
  });
  await expect(notice).toHaveAttribute("data-resolved", "true");
  await expect(notice.getByRole("status")).toHaveText("Every action the lock stopped has ended.");
  await expect(page.getByTestId("lock-report-omitted")).toHaveText("3 more actions, 7 more steps, 2 more wallets not listed here.");
});

test("a complete report of ended flows is resolved at once", async ({ page }) => {
  await page.goto(fixtureUrl("lock-report-resolved", "light"));
  await page.getByTestId("toolbar-lock").click();
  await expect(page.getByTestId("lock-report")).toHaveAttribute("data-resolved", "true");
  await expect(page.getByTestId("lock-report-omitted")).toHaveCount(0);
});

test("a lock that cut nothing shows no report", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light"));
  await page.getByTestId("toolbar-lock").click();
  await page.getByTestId("lock-screen").waitFor();
  await expect(page.getByTestId("lock-report")).toHaveCount(0);
});
