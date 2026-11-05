import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";

// 0.2 X1b slice 1 on the fixture backend: Receive on Platform, and Pay ▸ To an address paying a
// Platform address from the Platform balance (quote with the fee band, Max, confirm, outcomes,
// the shared hold, refused recipients). The rules are unit-tested (platform/send, fixture/pending).

const PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const SHIELDED = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero").waitFor();
  return errors;
}

async function payTo(page: Page, address: string) {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  await page.getByTestId("pay-address").fill(address);
  await page.locator("[data-testid=platform-send-intro], [data-testid=platform-send-empty]").first().waitFor();
}

test.describe("Receive on Platform", () => {
  test("is collapsed, and shows the bare Platform address with a QR and Copy once opened", async ({ page }) => {
    const errors = await open(page, "funded");
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("receive-qr").waitFor();
    await expect(page.getByTestId("platform-receive-address")).toBeHidden();
    await page.getByTestId("platform-receive-toggle").click();
    await expect(page.getByTestId("platform-receive-address")).toHaveText("tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0");
    await expect(page.getByTestId("platform-receive-qr")).toBeVisible();
    await expect(page.getByTestId("platform-receive-copy")).toBeEnabled();
    await expect(page.getByTestId("platform-receive-warning")).toContainText("cannot pay this address");
    // One read of the Platform addresses when it opens.
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_addresses_sync").length)).toBe(1);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("is not shown while the build has no Platform addresses", async ({ page }) => {
    await page.goto(fixtureUrl("pools-absent", "light"));
    await page.getByTestId("hero").waitFor();
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("receive-qr").waitFor();
    await expect(page.getByTestId("platform-receive")).toHaveCount(0);
  });
});

test.describe("Pay from Platform", () => {
  test("pays a Platform address: the fee and its band, the confirm, the result and Activity", async ({ page }) => {
    const errors = await open(page, "funded");
    await payTo(page, PAYEE);
    await expect(page.getByTestId("platform-send-intro")).toBeVisible();
    await expect(page.getByTestId("pay-label")).toHaveCount(0);
    await expect(page.getByTestId("pay-advanced")).toHaveCount(0);
    await page.getByTestId("pay-amount").fill("0.01");
    await expect(page.getByTestId("pay-fee-line")).toHaveText("Platform fee about 0.00006500 tDASH (up to 0.00019500 tDASH)");
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("platform-send-confirm-to")).toHaveText(PAYEE);
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.01000000 tDASH");
    await expect(page.getByTestId("platform-send-confirm-fee")).toHaveText("about 0.00006500 tDASH, up to 0.00019500 tDASH");
    await expect(page.getByTestId("platform-send-confirm-total")).toHaveText("0.01006500 tDASH");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("0.01006500 tDASH left your Platform balance");
    await page.getByTestId("platform-send-done").click();
    await expect(page.getByTestId("pay-address")).toHaveValue("");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Sent to a Platform address")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("Max sends everything and says the fee comes out of the amount", async ({ page }) => {
    await open(page, "funded");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-max").click();
    await page.getByTestId("pay-fee-line").waitFor();
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.40000000 tDASH");
    await expect(page.getByTestId("platform-send-fee-from-amount")).toBeVisible();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    // The send emptied the balance: the result stays until it is closed.
    await page.getByTestId("platform-send-done").click();
    await expect(page.getByTestId("platform-send-sheet")).toHaveCount(0);
  });

  test("an unanswered send waits for Platform, and holds the next send and Move until it settles", async ({ page }) => {
    await open(page, "platform-send-pending");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Waiting for Platform");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("Do not pay again");
    await page.getByTestId("platform-send-done").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await expect(page.getByTestId("pay-problem")).toContainText("not confirmed yet");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("platform-move")).toHaveCount(0);
    await expect(page.getByTestId("platform-move-view")).toBeVisible();
  });

  test("a send that did not go through in time says it may still go through", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Did not go through");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("may still go through");
    await expect(page.getByTestId("platform-send-view")).toBeVisible();
    // Only its exact retry may follow: a payment to someone else is refused.
    await page.getByTestId("platform-send-done").click();
    await page.getByTestId("pay-address").fill("tdash1sq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h");
    await page.getByTestId("platform-send-intro").waitFor();
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-refusal")).toContainText("only that same payment");
  });

  test("a held send keeps Pay from Platform back", async ({ page }) => {
    await open(page, "platform-send-held");
    await payTo(page, PAYEE);
    await expect(page.getByTestId("pay-problem")).toContainText("not confirmed yet");
    await expect(page.getByTestId("platform-send-waiting-view")).toBeVisible();
  });

  test("an empty Platform balance says paying from the Dash wallet is not supported", async ({ page }) => {
    await open(page, "platform-empty");
    await payTo(page, PAYEE);
    await expect(page.getByTestId("platform-send-empty")).toHaveText(/Platform balance is empty/);
    await expect(page.getByTestId("pay-amount")).toHaveCount(0);
  });

  test("refuses one of the wallet's own addresses and an amount below the minimum; a shielded address stays refused", async ({ page }) => {
    await open(page, "funded");
    await payTo(page, "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0");
    await page.getByTestId("pay-amount").fill("0.01");
    await expect(page.getByTestId("pay-problem")).toHaveText("This is one of your own Platform addresses.");
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.000001");
    await expect(page.getByTestId("pay-problem")).toContainText("must be at least");
    await page.getByTestId("pay-address").fill(SHIELDED);
    await expect(page.getByText("This is a shielded address.")).toBeVisible();
    await expect(page.getByTestId("platform-send-intro")).toHaveCount(0);
  });
});
