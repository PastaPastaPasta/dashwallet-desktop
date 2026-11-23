import { expect, test, type Page } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// Issue #141: while the wallet is still catching up, a payment the engine learned live (with its
// InstantSend lock) is announced on Receive, in a toast and on the bell; one the catch-up scan found
// in an old block is not.

async function openReceive(page: Page, theme: (typeof THEMES)[number]) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("catching-up-payment", theme));
  await expect(page.getByTestId("status-bar")).toContainText("Syncing");
  await page.getByTestId("sidebar-receive").click();
  await expect(page.getByTestId("receive-qr")).toBeVisible();
  return errors;
}

const noticeCalls = (page: Page) => page.evaluate(`window.__dwFixture.calls.filter((c) => c === "tx_notices").length`) as Promise<number>;
const historyRows = (page: Page) => page.evaluate(`window.__dwFixture.world.wallets[0].rows.length`) as Promise<number>;

const pay = (page: Page, duffs: number, to: string, source: "live" | "scanned") =>
  page.evaluate(`window.__dwFixture.receivePayment(${duffs}, undefined, ${JSON.stringify(to)}, ${JSON.stringify(source)})`);

for (const theme of THEMES) {
  test(`a payment found by the scan during sync is not announced; a live one is (${theme})`, async ({ page }, info) => {
    const errors = await openReceive(page, theme);
    const address = page.getByTestId("receive-address");
    const paidTo = (await address.textContent())!;
    const rows = await historyRows(page);
    await pay(page, 70_000_000, paidTo, "scanned");
    await expect.poll(() => historyRows(page)).toBe(rows + 1);
    await page.waitForTimeout(500);
    expect(await noticeCalls(page)).toBe(0);
    await expect(page.getByTestId("receive-received")).toHaveCount(0);
    await expect(page.locator(".toast")).toHaveCount(0);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);

    // The next address read waits until released, so the Received line stays for the checks.
    const shown = (await address.textContent())!;
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("current_receive_address"));
    await pay(page, 150_000_000, shown, "live");
    await expect(page.getByTestId("receive-received")).toHaveText("✓ Received 1.50 tDASH");
    await expect(page.locator(".toast")).toHaveText("Received 1.50 tDASH");
    await expect(page.getByTestId("toolbar-bell-badge")).toBeVisible();
    await expect(page.getByTestId("status-bar")).toContainText("Syncing");
    expect(await noticeCalls(page)).toBe(1);
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-receive-live-payment-during-sync.png`) });
    await release.evaluate((r) => r());
    expect(errors).toEqual([]);
  });
}
