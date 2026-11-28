import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

// QT-019: a `dash:` link dropped on the window opens Pay filled in, as File ▸ Open URI… would; a link
// it can't open says why in a toast; a link dropped into a text field is typed there.

async function open(page: Page) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", "light"));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

/** Drags `text` as a link (a browser's address-bar drag: a URI list and its plain text) onto `selector`. */
async function dropLink(page: Page, selector: string, text: string) {
  const data = await page.evaluateHandle((t) => {
    const d = new DataTransfer();
    d.setData("text/uri-list", t);
    d.setData("text/plain", t);
    return d;
  }, text);
  await page.dispatchEvent(selector, "dragover", { dataTransfer: data });
  await page.dispatchEvent(selector, "drop", { dataTransfer: data });
}

test("a dash: link dropped on the window opens Pay with it; one Dash can't read says why", async ({ page }) => {
  const errors = await open(page);
  await dropLink(page, '[data-testid="hero"]', `dash:${PAYEE}?amount=0.25&label=Coffee`);
  await expect(page.getByTestId("pay-address")).toHaveValue(PAYEE);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.25000000");
  await expect(page.getByTestId("pay-label")).toHaveValue("Coffee");

  await page.getByTestId("sidebar-home").click();
  await dropLink(page, '[data-testid="hero"]', `dash://${PAYEE}`);
  await expect(page.locator(".toast")).toContainText("starts with dash://");
  expect(errors).toEqual([]);
});

test("a contact or invitation link dropped on the window is not routed (it could be saved); a drop during setup isn't either", async ({ page }) => {
  await open(page);
  const routed = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "route_link").length);
  const before = await routed();
  await dropLink(page, '[data-testid="hero"]', "dashpay:contact?du=alice");
  await dropLink(page, '[data-testid="hero"]', "dashwallet:invite?du=bob");
  expect(await routed()).toBe(before);
  await page.goto(fixtureUrl("no-wallet", "light"));
  await page.getByTestId("onboarding-welcome").waitFor();
  const atSetup = await routed();
  await dropLink(page, '[data-testid="onboarding"]', `dash:${PAYEE}`);
  expect(await routed()).toBe(atSetup);
});

test("a link dropped into a text field is typed there, not opened", async ({ page }) => {
  await open(page);
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-message").fill("");
  const before = await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "route_link").length);
  await dropLink(page, '[data-testid="pay-message"]', `dash:${PAYEE}`);
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "route_link").length)).toBe(before);
});
