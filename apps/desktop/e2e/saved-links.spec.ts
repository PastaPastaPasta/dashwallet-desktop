import { expect, test, type Page } from "@playwright/test";
import type { LinkRoute } from "../src/api/generated/LinkRoute";
import type { World } from "../src/fixture/scenarios";
import { MAIN_PHRASE } from "../src/fixture/vault";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP5-01 on the fixture backend: a link opened before any wallet exists is kept (the toast says
// so), survives onboarding and is offered in "Links you opened": a payment opens Pay once, an
// invitation opens Join DashPay and stays until removed. The offer comes again after an unlock,
// once per trigger; after onboarding an invitation opens its claim screen first (DP5-03 D2).
// dw-app's intake and the vault's stash are tested in Rust (ipc_tests,
// vault/links_tests.rs); here the fixture stands in for them.

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const PAY: LinkRoute = { kind: "pay", uri: { address: ADDRESS, amount: 25_000_000, label: "Coffee", message: null }, username: null };
const INVITATION: LinkRoute = { kind: "invitation", linkId: "" };

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.locator("[data-testid=onboarding], [data-testid=toolbar]").first().waitFor();
  return errors;
}

const deliver = (page: Page, routes: LinkRoute[]) => page.evaluate((r) => window.__dwFixture!.deliverLinks(r), routes);
const world = <T>(page: Page, read: (w: World) => T) =>
  page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;
const toast = (page: Page) => page.locator(".toast-host");

async function axe(page: Page) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
}

test("DP5-01 a link opened before any wallet survives onboarding and opens once", async ({ page }) => {
  const errors = await open(page, "no-wallet-vault");
  await page.getByTestId("onboarding-welcome").waitFor();
  await deliver(page, [PAY]);
  await expect(toast(page)).toHaveText("Link saved. It opens once your wallet is set up.");
  await deliver(page, [INVITATION]);
  await expect(toast(page)).toHaveText("Invitation saved. It is waiting for you once your wallet is set up.");
  expect(await world(page, (w) => w.stash.map((s) => s.info.kind))).toEqual(["payment", "invitation"]);
  await expect(page.getByTestId("saved-links")).toHaveCount(0);

  // Onboarding: restore the wallet; once it is open the saved links are offered.
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(MAIN_PHRASE);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await page.getByTestId("onboarding-open").click();
  // DP5-03 D2: the invitation goes first, on its claim screen; the sheet waits for the next offer.
  await expect(page.getByTestId("claim")).toHaveAttribute("data-entry", "welcome");
  const sheet = page.getByTestId("saved-links");
  await expect(sheet).toHaveCount(0);
  await page.getByTestId("claim-notNow").click();
  await expect(page.getByTestId("page-home")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(sheet).toBeVisible();
  await expect(sheet.getByTestId("saved-link")).toHaveCount(2);
  await expect(sheet.getByTestId("saved-link")).toContainText(["Payment request", "DashPay invitation"]);
  await axe(page);

  // The payment request opens Pay, prefilled, and leaves the stash.
  await sheet.getByTestId("saved-link").filter({ hasText: "Payment request" }).getByTestId("saved-link-open").click();
  await expect(sheet).toHaveCount(0);
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-label")).toHaveValue("Coffee");
  expect(await world(page, (w) => w.stash.map((s) => s.info.kind))).toEqual(["invitation"]);
  expect(errors).toEqual([]);
});

test("DP5-01 an invitation opens its claim screen (DP5-03), stays saved, and is offered again after an unlock", async ({ page }) => {
  await open(page, "saved-links");
  const sheet = page.getByTestId("saved-links");
  await expect(sheet.getByTestId("saved-link")).toHaveCount(3);
  await sheet.getByTestId("saved-link").filter({ hasText: "DashPay invitation" }).getByTestId("saved-link-open").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await expect(page.getByTestId("claim")).toHaveAttribute("data-state", "invited");
  await expect(page.getByTestId("claim")).toHaveAttribute("data-entry", "join");
  expect(await world(page, (w) => w.stash.length)).toBe(3);

  // Once per trigger: not again until the next unlock.
  await page.waitForTimeout(300);
  await expect(sheet).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(sheet).toBeVisible();

  // Remove forgets one; the last one closes the sheet.
  for (const left of [2, 1]) {
    await sheet.getByTestId("saved-link-remove").first().click();
    await expect(sheet.getByTestId("saved-link")).toHaveCount(left);
  }
  await sheet.getByTestId("saved-link-remove").click();
  await expect(sheet).toHaveCount(0);
  expect(await world(page, (w) => w.stash.length)).toBe(0);
});

test("DP5-01 Later closes the offer; a link already taken says so", async ({ page }) => {
  await open(page, "saved-links");
  const sheet = page.getByTestId("saved-links");
  await expect(sheet).toBeVisible();
  await page.getByTestId("saved-links-later").click();
  await expect(sheet).toHaveCount(0);
  expect(await world(page, (w) => w.stash.length)).toBe(3);

  // The entry went away meanwhile (taken in another window, expired): the sheet says so.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await world(page, (w) => w.stash.splice(0, w.stash.length - 1));
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(sheet.getByTestId("saved-link")).toHaveCount(1);
  await world(page, (w) => (w.stash.length = 0));
  await sheet.getByTestId("saved-link-open").click();
  await expect(toast(page)).toHaveText("This link is no longer saved.");
});

for (const theme of THEMES) {
  test(`DP5-01 Links you opened ${theme}: 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "saved-links", theme);
    await expect(page.getByTestId("saved-links").getByTestId("saved-link")).toHaveCount(3);
    await axe(page);
    expect(errors).toEqual([]);
  });
}
