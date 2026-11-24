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
  await expect(page.getByTestId("pay-from-shielded-reason")).toHaveText("Not available on this network yet.");
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
  await page.getByTestId("pay-balance-go").click();
  await authorize(page);
  await expect(page.getByTestId("pay-balance-outcome-title")).toHaveText("Payment sent");
  await expect(page.getByTestId("pay-balance-outcome-text")).toHaveText("Payment sent. It arrives in Bob Builder’s wallet once the Dash network processes it. This can take a while.");
  await page.getByTestId("pay-balance-done").click();
  await expect(page.getByTestId("pay-amount")).toHaveValue("");
  expect(errors).toEqual([]);
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
