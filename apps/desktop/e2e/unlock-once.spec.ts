import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// A passphrase unlock covers a second send under the spend limit. One over the limit, and one
// after the window has ended, asks again. The funded scenario leaves "ask every payment" on;
// these tests turn it off, which is the engine's default.

const world = <T,>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function open(page: Page, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("toolbar").waitFor();
  await world(page, (w) => {
    w.paymentAuth = false;
  });
  return errors;
}

async function review(page: Page, amount: string) {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-address").fill(PAYEE);
  await page.getByTestId("pay-amount").fill(amount);
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
}

test("a second send inside the window does not ask, and one over the limit does", async ({ page }) => {
  const errors = await open(page);
  await review(page, "0.1");
  await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await page.getByTestId("pay-result-close").click();

  await review(page, "0.1");
  await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
  await page.getByTestId("pay-confirm-cancel").click();

  await review(page, "1");
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeVisible();
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(errors).toEqual([]);
});

test("the limit is a total for the window, not per payment", async ({ page }) => {
  const errors = await open(page);
  await review(page, "0.3");
  await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await page.getByTestId("pay-result-close").click();

  // A second 0.3 would add up past the 0.5 limit.
  await review(page, "0.3");
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeVisible();
  expect(errors).toEqual([]);
});

test("sleep and lock end the window", async ({ page }) => {
  await open(page);
  await world(page, (w) => {
    w.signingUntil = 0;
  });
  await review(page, "0.1");
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeVisible();
  await page.getByTestId("pay-confirm-cancel").click();

  await page.getByTestId("toolbar-lock").click();
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await page.getByTestId("lock-field").fill("demo");
  await page.getByTestId("lock-field").press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await review(page, "0.1");
  await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
});
