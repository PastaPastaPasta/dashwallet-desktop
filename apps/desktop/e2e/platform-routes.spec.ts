import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";

// 0.2 X1b slice 1 on the fixture backend: Receive on Platform, and Pay ▸ To an address paying a
// Platform address from the Platform balance (quote with the fee bound, Max, confirm, outcomes,
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
    await expect(page.getByTestId("pay-fee-line")).toHaveText("Platform fee usually about 0.00006500 tDASH");
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("platform-send-confirm-to")).toHaveText(PAYEE);
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.01000000 tDASH");
    await expect(page.getByTestId("platform-send-confirm-fee")).toHaveText("usually about 0.00006500 tDASH");
    await expect(page.getByTestId("platform-send-confirm-fee-source")).not.toBeEmpty();
    await expect(page.getByTestId("platform-send-confirm-total")).toHaveText("0.01006500 tDASH");
    await expect(page.getByTestId("platform-send-retry-note")).toHaveCount(0);
    await expect(page.getByTestId("platform-send-fee-source")).toContainText("it can be more: up to everything that address holds then (0.29000000 tDASH after this payment, plus anything sent to it later)");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("About 0.01006500 tDASH left your Platform balance");
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
    await expect(page.getByTestId("pay-fee-line")).toHaveText("Platform fee usually about 0.00006500 tDASH, from the amount");
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.40000000 tDASH, everything");
    await expect(page.getByTestId("platform-send-confirm-fee")).toHaveText("usually about 0.00006500 tDASH");
    await expect(page.getByTestId("platform-send-confirm-fee-source")).toHaveCount(0);
    await expect(page.getByTestId("platform-send-confirm-recipient")).toHaveText("0.39993500 tDASH");
    await expect(page.getByTestId("platform-send-confirm-total")).toHaveText("0.40000000 tDASH");
    await expect(page.getByText("Recipient gets, about")).toBeVisible();
    await expect(page.getByTestId("platform-send-fee-from-amount")).toContainText("which may be more than the estimate");
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

  test("a send that did not go through in time says it may still go through, keeps the form, and shows in Activity", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Did not go through");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("may still go through");
    await expect(page.getByTestId("platform-send-view")).toBeVisible();
    await expect(page.getByTestId("platform-send-retry")).toHaveText("Retry this payment");
    // The form keeps what was typed.
    await page.getByTestId("platform-send-done").click();
    await expect(page.getByTestId("pay-address")).toHaveValue(PAYEE);
    await expect(page.getByTestId("pay-amount")).toHaveValue("0.01");
    // Activity shows it as unconfirmed, never as not accepted.
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Sent to a Platform address")).toBeVisible();
    await expect(page.getByText("May still go through")).toBeVisible();
    await expect(page.getByText("Not accepted")).toHaveCount(0);
  });

  test("a payment that is not the held one is refused on the form, before any passphrase", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Did not go through");
    await page.getByTestId("platform-send-done").click();
    await page.getByTestId("pay-address").fill("tdash1sq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h");
    await page.getByTestId("platform-send-intro").waitFor();
    await expect(page.getByTestId("pay-problem")).toContainText("Wait for it to finish, or press Retry that payment");
    await expect(page.getByTestId("pay-continue")).toBeDisabled();
  });

  test("Retry this payment quotes the same payment again and sends it once", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Did not go through");
    await page.getByTestId("platform-send-retry").click();
    await expect(page.getByTestId("platform-send-confirm-to")).toHaveText(PAYEE);
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.01000000 tDASH");
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_send").length)).toBe(2);
    await page.getByTestId("platform-send-done").click();
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Replaced by a retry")).toBeVisible();
    await expect(page.getByText("May still go through")).toHaveCount(0);
  });

  test("a held send that failed by height shows as 'May still go through' in Activity, also in a fresh session", async ({ page }) => {
    await open(page, "platform-send-failed-may-land");
    const activity = async () => {
      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      await expect(page.getByText("Sent to a Platform address")).toBeVisible();
      await expect(page.getByText("May still go through")).toBeVisible();
      await expect(page.getByText("Not accepted")).toHaveCount(0);
    };
    await activity();
    await page.reload();
    await page.getByTestId("hero").waitFor();
    await activity();
  });

  test("the held send's exact retry is quoted and sent once from the form", async ({ page }) => {
    await open(page, "platform-send-failed-may-land");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-fee-line").waitFor();
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_send").length)).toBe(1);
  });

  test("Retry this payment after the held send landed pays once and says the payment went through", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Did not go through");
    // Platform takes the held payment after all, before Activity has been read again.
    const credits = () => page.evaluate(() => window.__dwFixture!.world.pending.platform!.addresses.reduce((n, a) => n + BigInt(a.credits), 0n).toString());
    await page.evaluate(() => {
      const p = window.__dwFixture!.world.pending.platform!;
      window.__dwFixture!.landPlatformSend(p.activity.find((a) => a.kind === "send")!.id);
    });
    expect(await credits()).toBe("38993500000");
    await page.getByTestId("platform-send-retry").click();
    await expect(page.getByTestId("platform-send-retry-note")).toHaveText("This retries your earlier payment. If that one went through, nothing more is sent.");
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    await expect(page.getByTestId("platform-send-outcome-text")).toContainText("That payment went through.");
    // Paid once: the recipient's debit was taken by the landed send only.
    expect(await credits()).toBe("38993500000");
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.filter((a) => a.kind === "send").map((a) => a.state))).toEqual(["done"]);
  });

  test("a result sheet follows Activity: the held payment went through, so no Retry is offered", async ({ page }) => {
    await open(page, "platform-send-failed");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-retry")).toBeVisible();
    await page.evaluate(() => {
      const p = window.__dwFixture!.world.pending.platform!;
      window.__dwFixture!.landPlatformSend(p.activity.find((a) => a.kind === "send")!.id);
      // The engine's signal that Platform activity changed.
      const w = window.__dwFixture!.world;
      window.__dwFixture!.emit({ kind: "platform", network: w.launch.network.dirName, walletId: w.wallets[0].summary.walletId, change: { domain: "platformAddresses" } } as never);
    });
    await expect(page.getByTestId("platform-send-outcome-text")).toHaveText("That payment went through. Activity shows it.");
    await expect(page.getByTestId("platform-send-retry")).toHaveCount(0);
  });

  test("the form offers Retry that payment for an earlier payment that may still go through, also after a reload", async ({ page }) => {
    await open(page, "platform-send-failed-may-land");
    await payTo(page, PAYEE);
    await expect(page.getByTestId("platform-send-earlier")).toContainText(`An earlier payment of 0.01000000 tDASH to ${PAYEE} may still go through.`);
    await page.reload();
    await page.getByTestId("hero").waitFor();
    await payTo(page, PAYEE);
    await expect(page.getByTestId("platform-send-earlier")).toContainText("may still go through");
    await page.getByTestId("platform-send-retry-held").click();
    await expect(page.getByTestId("platform-send-confirm-to")).toHaveText(PAYEE);
    await expect(page.getByTestId("platform-send-confirm-amount")).toHaveText("0.01000000 tDASH");
    await expect(page.getByTestId("platform-send-retry-note")).toBeVisible();
    await page.getByTestId("platform-send-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-send-outcome-title")).toHaveText("Sent");
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_send").length)).toBe(1);
    await page.getByTestId("platform-send-done").click();
    await expect(page.getByTestId("platform-send-earlier")).toHaveCount(0);
  });

  test("a held move leaves Move offered, and moving again goes through", async ({ page }) => {
    await open(page, "platform-move-failed-may-land");
    await expect(page.getByTestId("platform-move")).toBeEnabled();
    await expect(page.getByTestId("platform-move-view")).toHaveCount(0);
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
  });

  test("a held send says so on the card, and the move's quote refuses before any passphrase", async ({ page }) => {
    await open(page, "platform-send-failed-may-land");
    await expect(page.getByTestId("platform-card-send-held")).toHaveText("A payment from Platform may still go through. Moving waits until it settles.");
    await page.getByTestId("platform-move").click();
    await expect(page.getByTestId("platform-move-quote-error")).toHaveText("An earlier payment or move from Platform may still go through. Wait for it to finish, then move again.");
    await expect(page.getByTestId("platform-move-go")).toBeDisabled();
    await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_move_to_core").length)).toBe(0);
  });

  test("a held move refuses Pay from Platform with the move's own way out", async ({ page }) => {
    await open(page, "platform-move-failed-may-land");
    await payTo(page, PAYEE);
    await page.getByTestId("pay-amount").fill("0.01");
    await expect(page.getByTestId("pay-problem")).toHaveText("A move to your Dash wallet may still go through. Wait for it to finish, or move again from the Platform card on Home.");
    await expect(page.getByTestId("platform-send-earlier")).toHaveCount(0);
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
