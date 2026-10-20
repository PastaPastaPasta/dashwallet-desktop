import { expect, test, type Locator, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors } from "./helpers";

// DP4-03 D12–D14: one avatar in every list (contacts and requests, the contact pane, history
// rows, the frequent strips, the pay-to-contact picker, the bell, the identity chip), each the
// identity's verified thumbnail or its initials; "Load contact pictures" off shows only the
// wallet's own, and the Settings switch takes effect at once. IOS-076's picture shown as the
// lists show it; DASHPAY F15.

async function open(page: Page, scenario = "dashpay-avatars") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const go = (page: Page, route: "contacts" | "pay" | "profile" | "settings") =>
  page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);
const pictured = (l: Locator) => l.locator(".contact-avatar[data-picture], .identity-chip-avatar[data-picture]");
const initialsOnly = (l: Locator) => l.locator(".contact-avatar:not([data-picture])");
const contactRow = (page: Page, name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });

test("F15 contacts, requests and the pane show each identity's picture, or its initials", async ({ page }) => {
  const errors = await open(page);
  await go(page, "contacts");
  await expect(pictured(contactRow(page, "Bob Builder"))).toHaveCount(1);
  await expect(pictured(contactRow(page, "Grace Hopper"))).toHaveCount(1);
  // Carol has no picture; Erin's does not decode, so her initials stay.
  await expect(initialsOnly(contactRow(page, "Carol"))).toHaveText("C");
  await expect(initialsOnly(contactRow(page, "Erin"))).toHaveText("E");
  await contactRow(page, "Bob Builder").click();
  await expect(pictured(page.getByTestId("contact-detail"))).not.toHaveCount(0);
  expect(errors).toEqual([]);
});

test("F15 history rows and the home frequent strip show pictures", async ({ page }) => {
  await open(page);
  await page.getByTestId("hero").waitFor();
  await expect(pictured(page.getByTestId("home-frequent-chip").first())).toHaveCount(1);
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  await expect(pictured(page.getByTestId("tx-list")).first()).toBeVisible();
});

test("F15 the pay-to-contact picker, its frequent strip and the payee show pictures", async ({ page }) => {
  await open(page);
  await go(page, "pay");
  const bob = page.getByTestId("pay-picker-row").filter({ has: page.locator(".contact-row-name", { hasText: "Bob Builder" }) });
  await expect(pictured(bob)).toHaveCount(1);
  await expect(pictured(page.getByTestId("pay-frequent-chip").first())).toHaveCount(1);
  await bob.click();
  await expect(pictured(page.getByTestId("pay-payee"))).toHaveCount(1);
});

test("F15 the bell's requests and events show pictures; the chip shows the own one", async ({ page }) => {
  await open(page);
  await expect(pictured(page.getByTestId("toolbar-chip"))).toHaveCount(1);
  await page.getByTestId("toolbar-bell").click();
  const popover = page.getByTestId("bell-popover");
  await expect(pictured(popover.getByTestId("bell-request")).first()).toBeVisible();
  await expect(pictured(popover.getByTestId("bell-event")).first()).toBeVisible();
});

test("D13 with Load contact pictures off only the wallet's own picture shows", async ({ page }) => {
  const errors = await open(page, "dashpay-pictures-off");
  await expect(pictured(page.getByTestId("toolbar-chip"))).toHaveCount(1);
  await go(page, "contacts");
  await expect(page.getByTestId("contact-row").first()).toBeVisible();
  await expect(pictured(page.getByTestId("page-contacts"))).toHaveCount(0);
  await go(page, "pay");
  await expect(page.getByTestId("pay-picker-row").first()).toBeVisible();
  await expect(pictured(page.getByTestId("pay-contact"))).toHaveCount(0);
  await go(page, "profile");
  await expect(pictured(page.getByTestId("profile-header"))).toHaveCount(1);
  expect(errors).toEqual([]);
});

test("D14 the Settings switch shows and hides the pictures at once", async ({ page }) => {
  await open(page, "dashpay-pictures-off");
  await go(page, "settings");
  const toggle = page.getByTestId("settings-contact-pictures");
  await expect(toggle).not.toBeChecked();
  await toggle.check();
  await expect(toggle).toBeChecked();
  await go(page, "contacts");
  await expect(pictured(contactRow(page, "Bob Builder"))).toHaveCount(1);
  await go(page, "settings");
  await page.getByTestId("settings-contact-pictures").uncheck();
  await go(page, "contacts");
  await expect(initialsOnly(contactRow(page, "Bob Builder"))).toHaveText("BB");
  expect(await page.evaluate(() => window.__dwFixture!.world.loadContactPictures)).toBe(false);
});
