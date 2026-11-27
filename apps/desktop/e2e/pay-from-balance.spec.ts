import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Pay ▸ To a contact ▸ From (C-funding D-C5, P13–P16) on the fixture backend
// (src/fixture/payFromBalance.test.ts): the Dash wallet each time, each source with its balance or
// its reason, a payment from the Platform balance through its confirm and result, and the locked
// banner an unknown one leaves.

const world = <T>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function openBob(page: Page, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-frequent-chip").first().click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  await page.getByTestId("pay-from").waitFor();
  return errors;
}

async function reviewFromPlatform(page: Page, amount = "0.1") {
  await page.getByTestId("pay-from-platform").locator("input").check();
  await page.getByTestId("pay-amount").fill(amount);
  await page.getByTestId("pay-fee-line").waitFor();
  await page.getByTestId("pay-continue").click();
  await page.getByTestId("pay-balance-confirm").waitFor();
}

async function payFromPlatform(page: Page) {
  await reviewFromPlatform(page);
  await page.getByTestId("pay-balance-go").click();
  await authorize(page);
  await page.getByTestId("pay-balance-outcome-title").waitFor();
}

test("PAY-FROM the form opens on the Dash wallet, every source listed with its balance or reason", async ({ page }) => {
  const errors = await openBob(page);
  const from = page.getByTestId("pay-from");
  await expect(from.getByRole("radio", { name: /Dash wallet/ })).toBeChecked();
  await expect(page.getByTestId("pay-from-platform-balance")).toHaveText("0.40000000 tDASH");
  await expect(page.getByTestId("pay-from-shielded-reason")).toHaveText("Not available in this version");
  await expect(from.getByRole("radio", { name: /Shielded balance/ })).toBeDisabled();
  // The Dash wallet's form is the one it always was.
  await expect(page.getByTestId("pay-max")).toBeVisible();
  expect(await seriousAxeViolations(page)).toEqual([]);
  // Platform: no Max, no "subtract fee", no coin control.
  await page.getByTestId("pay-from-platform").locator("input").check();
  await expect(page.getByTestId("pay-max")).toHaveCount(0);
  await expect(page.getByTestId("pay-subtract-fee")).toHaveCount(0);
  // Never remembered: the next form opens on the Dash wallet again.
  await page.getByTestId("pay-payee-change").click();
  await page.getByTestId("pay-frequent-chip").first().click();
  await expect(page.getByTestId("pay-from").getByRole("radio", { name: /Dash wallet/ })).toBeChecked();
  expect(errors).toEqual([]);
});

test("PAY-FROM pays a contact from the Platform balance", async ({ page }) => {
  const errors = await openBob(page);
  await reviewFromPlatform(page);
  await expect(page.getByTestId("pay-balance-confirm")).toHaveText(
    "Pay Bob Builder 0.10000000 tDASH from your Platform balance. They receive about 0.09999774 tDASH. Network fee 0.00002000 tDASH.",
  );
  // What tells two contacts of one name apart, and the whole debit: the amount and the fee.
  await expect(page.getByTestId("pay-balance-payee")).toHaveText("Bob Builder@bob");
  await expect(page.getByTestId("pay-balance-total")).toHaveText("About 0.10002000 tDASH leaves your Platform balance: 0.10000000 tDASH to Bob Builder and about 0.00002000 tDASH in Platform fees.");
  await page.getByTestId("pay-balance-go").click();
  await authorize(page);
  await expect(page.getByTestId("pay-balance-outcome-title")).toHaveText("Payment sent");
  await expect(page.getByTestId("pay-balance-outcome-text")).toHaveText("Payment sent. It arrives in Bob Builder’s wallet once the Dash network processes it. This can take a while.");
  await page.getByTestId("pay-balance-done").click();
  await expect(page.getByTestId("pay-amount")).toHaveValue("");
  // The next payment opens on the Dash wallet again (P16).
  await expect(page.getByTestId("pay-from").getByRole("radio", { name: /Dash wallet/ })).toBeChecked();
  // Activity names who was paid, on the row itself (P10).
  await page.getByTestId("sidebar-activity").click();
  const row = page.locator('[data-txid^="platform:contactPayment:"]').first();
  await expect(row.locator(".tx-row-title")).toHaveText("Paid from Platform");
  await expect(row.locator(".tx-row-party")).toHaveText("Bob Builder");
  await expect(row.locator(".tx-row-party")).toBeVisible();
  expect(errors).toEqual([]);
});

test("PAY-FROM a lost reply keeps the contact held until a later read settles it", async ({ page }) => {
  const errors = await openBob(page);
  await reviewFromPlatform(page);
  // The payout is recorded and sent, but its reply is lost, and the reads that would settle it fail.
  await world(page, (w) => {
    w.pending.platform!.loseReply = true;
  });
  await page.evaluate(() => window.__dwFixture!.fail("platform_activity", "feature"));
  await page.getByTestId("pay-balance-go").click();
  await authorize(page);
  await expect(page.getByTestId("pay-balance-outcome-title")).toHaveText("Payment status unknown");
  await page.getByTestId("pay-balance-done").click();
  // No reference and no engine lock: the form still holds the contact, and Check again with the
  // read still failing settles nothing.
  await expect(page.getByTestId("pay-banner-text")).toHaveText("A payment to this contact from your Platform balance may have been sent.");
  await expect(page.getByTestId("pay-from")).toHaveCount(0);
  await expect(page.getByTestId("pay-amount")).toHaveCount(0);
  await page.getByTestId("pay-banner-check").click();
  await expect(page.getByTestId("pay-banner-check")).toBeEnabled();
  await expect(page.getByTestId("pay-banner")).toBeVisible();
  await expect(page.getByTestId("pay-from")).toHaveCount(0);
  // Check again after the read answers: the payout was recorded sent.
  await page.evaluate(() => window.__dwFixture!.fail("platform_activity", null));
  await page.getByTestId("pay-banner-check").click();
  await expect(page.getByTestId("pay-lock-resolve")).toHaveText("The payment was sent. You can pay this contact again.");
  await expect(page.getByTestId("pay-banner")).toHaveCount(0);
  await expect(page.getByTestId("pay-from")).toBeVisible();
  expect(errors).toEqual([]);
});

test("PAY-FROM a chosen balance that empties holds Review with its reason, never pays from the Dash wallet", async ({ page }) => {
  const errors = await openBob(page);
  await page.getByTestId("pay-from-platform").locator("input").check();
  await page.getByTestId("pay-amount").fill("0.1");
  await page.getByTestId("pay-fee-line").waitFor();
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.pending.platform!.addresses = f.world.pending.platform!.addresses.map((a) => ({ ...a, credits: "0" }));
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "startup" } });
  });
  await expect(page.getByTestId("pay-from-platform-reason")).toHaveText("No balance to pay with.");
  await expect(page.getByTestId("pay-from").getByRole("radio", { name: /Platform balance/ })).toBeChecked();
  await expect(page.getByTestId("pay-problem")).toHaveText("No balance to pay with.");
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  // Still the Platform form: no Max or coin control of the Dash wallet's.
  await expect(page.getByTestId("pay-max")).toHaveCount(0);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.1");
  expect(errors).toEqual([]);
});

test("PAY-FROM the form quotes again after a refused attempt and once a held transfer settles", async ({ page }) => {
  await openBob(page);
  const quotes = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "contact_pay_quote").length);
  // A transfer holds the balance: the quote is refused until it settles, with no edit of the amount.
  await world(page, (w) => {
    const p = w.pending.platform!;
    p.activity = [{ id: "held-1", kind: "moveToCore", state: "pending", credits: "1000000000", address: null, to: "yheld", expectedDuffs: null, feeCredits: null, at: 1, mayLand: true, estimated: false }, ...p.activity];
  });
  await page.getByTestId("pay-from-platform").locator("input").check();
  await page.getByTestId("pay-amount").fill("0.1");
  await expect(page.getByTestId("pay-problem")).toHaveText("Another Platform transfer is still being confirmed. Try again when it is done.");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.pending.platform!.activity = f.world.pending.platform!.activity.filter((a) => a.id !== "held-1");
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "startup" } });
  });
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  // A refused attempt (a grant too small) closes to a new quote, not the one it was refused under.
  await page.evaluate(() => window.__dwFixture!.fail("contact_pay", "budget"));
  await page.getByTestId("pay-continue").click();
  await page.getByTestId("pay-balance-go").click();
  await authorize(page);
  await expect(page.getByTestId("pay-balance-refusal")).toBeVisible();
  const before = await quotes();
  await page.getByTestId("pay-balance-close").click();
  await expect.poll(quotes).toBeGreaterThan(before);
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
});

test("PAY-FROM discreet mode masks the amounts a refusal names", async ({ page }) => {
  await openBob(page);
  await page.getByTestId("toolbar-discreet").click();
  await page.getByTestId("pay-from-platform").locator("input").check();
  await page.getByTestId("pay-amount").fill("0.5");
  await expect(page.getByTestId("pay-problem")).toBeVisible();
  await expect(page.getByTestId("pay-problem")).not.toContainText("0.4");
  await expect(page.getByTestId("pay-problem")).not.toContainText("0.5");
});

test("PAY-FROM below the minimum names it and points to the Dash wallet", async ({ page }) => {
  await openBob(page);
  await page.getByTestId("pay-from-platform").locator("input").check();
  await page.getByTestId("pay-amount").fill("0.000001");
  await expect(page.getByTestId("pay-problem")).toHaveText("The smallest payment from here is 0.00001190 tDASH. Pay a smaller amount from your Dash wallet.");
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
});

test("PAY-FROM an unknown payment locks the contact with the Platform banner", async ({ page }) => {
  const errors = await openBob(page);
  await world(page, (w) => {
    w.pending.platform!.nextPayout = "pending";
  });
  await payFromPlatform(page);
  await expect(page.getByTestId("pay-balance-outcome-title")).toHaveText("Payment status unknown");
  await expect(page.getByTestId("pay-balance-outcome-text")).toHaveText("We could not confirm this payment. It may still go through. Do not pay again until this is settled.");
  await page.getByTestId("pay-balance-done").click();
  await expect(page.getByTestId("pay-banner-text")).toHaveText("A payment to this contact from your Platform balance may have been sent.");
  await expect(page.getByTestId("pay-banner-check")).toBeVisible();
  // A payout's reference is no Core transaction.
  await expect(page.getByTestId("pay-banner-show")).toHaveCount(0);
  await expect(page.getByTestId("pay-from")).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const theme of THEMES) {
  test(`PAY-FROM screenshots ${theme}`, async ({ page }, info) => {
    await openBob(page, theme);
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-form.png`), fullPage: true });
    await page.getByTestId("pay-from-platform").locator("input").check();
    await page.getByTestId("pay-amount").fill("0.1");
    await page.getByTestId("pay-fee-line").waitFor();
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-platform.png`), fullPage: true });
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("pay-balance-confirm").waitFor();
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-confirm.png`), fullPage: true });
    await page.getByTestId("pay-balance-go").click();
    await authorize(page);
    await page.getByTestId("pay-balance-outcome-title").waitFor();
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-sent.png`), fullPage: true });
    await page.getByTestId("pay-balance-done").click();
    await page.getByTestId("pay-from-platform").locator("input").check();
    await world(page, (w) => {
      w.pending.platform!.nextPayout = "pending";
    });
    await page.getByTestId("pay-amount").fill("0.1");
    await page.getByTestId("pay-fee-line").waitFor();
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("pay-balance-go").click();
    await authorize(page);
    await page.getByTestId("pay-balance-outcome-title").waitFor();
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-unknown.png`), fullPage: true });
    await page.getByTestId("pay-balance-done").click();
    await page.getByTestId("pay-banner").waitFor();
    await page.screenshot({ path: artifactPath(info, `pay-from-${theme}-locked.png`), fullPage: true });
  });
}
