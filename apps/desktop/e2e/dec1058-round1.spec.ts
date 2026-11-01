import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl } from "./helpers";

// DEC-1058 fix round 1 (Sol's review): the cases the first batch's tests did not reach. Each test
// here failed on d74b2dab.

/** The toast region's text right now: no retry. */
const toastText = (page: Page) => page.locator(".toast-host").evaluate((e) => e.textContent ?? "");

const pay = (page: Page, duffs: number) => page.evaluate(`window.__dwFixture.receivePayment(${duffs})`) as Promise<string>;

async function open(page: Page, scenario = "funded") {
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero").waitFor();
}

test.describe("payment toast privacy", () => {
  test("a toast already on screen is masked when Discreet turns on", async ({ page }) => {
    await open(page);
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received a payment");
    expect(await toastText(page)).not.toContain("0.00999819");
    // And back: the toast is the same one, not a new announcement.
    await page.getByTestId("toolbar-discreet").click();
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
  });

  test("a toast on screen is dismissed when the wallet locks", async ({ page }) => {
    await open(page);
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
    await page.getByTestId("toolbar-lock").click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    // Read now, not retried: a retrying assertion would pass once the toast timed out by itself.
    expect(await toastText(page)).toBe("");
    // A payment that arrives while locked is not announced either.
    await pay(page, 150_000_000);
    await page.waitForTimeout(400);
    expect(await toastText(page)).toBe("");
    await authorizeUnlock(page);
  });

  test("a payment that arrives in Discreet mode is masked from the start", async ({ page }) => {
    await open(page);
    await page.getByTestId("toolbar-discreet").click();
    await pay(page, 999_819);
    await expect(page.locator(".toast")).toHaveText("Received a payment");
  });
});

async function authorizeUnlock(page: Page) {
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await page.getByTestId("hero").waitFor();
}

void authorize;
