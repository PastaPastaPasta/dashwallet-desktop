import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl, trackPageErrors } from "./helpers";

// DP2-09: the selected contact's pane on the fixture backend, whose DashPay writes check their
// grant as the engine does (src/fixture/pane.test.ts). Tests are named by parity ID: IOS-074 (a
// contact's actions and private details), AND-093 (its payment activity), IOS-072 (Enable
// DashPay). The axe runs are contacts.spec.ts's `pane-*` and `enable-banner` states.

async function openContacts(page: Page, scenario = "dashpay-registered") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  await expect(page.getByTestId("page-contacts").locator("h1")).toBeFocused();
  return errors;
}

const group = (page: Page, section: string) => page.getByTestId(`contacts-group-${section}`);
const names = (page: Page, section: string) => group(page, section).locator(".contact-row-name");
const row = (page: Page, name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
const rowItem = (page: Page, name: string) => page.locator("li").filter({ has: row(page, name) });

test("IOS-074 accepts a request from the pane under a passphrase grant; the contact moves to My Contacts", async ({ page }) => {
  const errors = await openContacts(page);
  await row(page, "heidi").click();
  await expect(page.getByTestId("contacts-announcer")).toHaveText("heidi, Request received");
  await expect(page.getByTestId("contact-pay")).toHaveCount(0);
  await page.getByTestId("contact-accept").click();
  await expect(page.getByTestId("contact-accept")).toHaveText("Accepting…");
  await expect(page.getByTestId("grant-sheet")).toContainText("authorize this Dash Platform operation");
  await authorize(page);
  await expect(page.getByTestId("contact-pane-outcome")).toHaveText("You and heidi are now contacts.");
  await expect(names(page, "requests")).toHaveText(["Grace Hopper"]);
  await expect(names(page, "contacts")).toContainText(["heidi"]);
  // Now a contact: Pay waits for the payment channel, and the private details show.
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Contact");
  await expect(page.getByTestId("contact-pay")).toBeDisabled();
  await expect(page.getByTestId("contact-private")).toBeVisible();
  expect(errors).toEqual([]);
});

test("D3 a cancelled passphrase sends nothing; the request stays", async ({ page }) => {
  await openContacts(page);
  await row(page, "heidi").click();
  await page.getByTestId("contact-accept").click();
  await page.getByTestId("grant-sheet").waitFor();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  await expect(page.getByTestId("contact-accept")).toHaveText("Accept");
  await expect(page.getByTestId("contact-pane-outcome")).toHaveCount(0);
  await expect(names(page, "requests")).toHaveText(["heidi", "Grace Hopper"]);
});

test("D15 a request row's Ignore works without selecting it, and a toast says so; Unignore brings it back", async ({ page }) => {
  await openContacts(page);
  await expect(rowItem(page, "Grace Hopper").getByTestId("contact-row-accept")).toHaveAccessibleName("Accept Grace Hopper");
  await rowItem(page, "Grace Hopper").getByTestId("contact-row-ignore").click();
  await expect(page.locator(".toast")).toHaveText("Request ignored. You can find it under Hidden.");
  await expect(names(page, "requests")).toHaveText(["heidi"]);
  await expect(page.getByTestId("contact-pane-empty")).toBeVisible();

  await group(page, "hidden").getByRole("button").click();
  await row(page, "Grace Hopper").click();
  await expect(page.getByTestId("contact-pane-note")).toHaveText("You ignored this contact request.");
  await page.getByTestId("contact-unignore").click();
  await expect(page.getByTestId("contact-pane-outcome")).toHaveText("The request is back in Contact Requests.");
  await expect(names(page, "requests")).toHaveText(["heidi", "Grace Hopper"]);
  await expect(page.getByTestId("contact-accept")).toBeEnabled();
});

test("IOS-074 saves an alias and a note under a grant; Hide saves at once and moves the contact to Hidden", async ({ page }) => {
  const errors = await openContacts(page);
  await row(page, "Bob Builder").click();
  const save = page.getByTestId("contact-details-save");
  await expect(save).toBeDisabled();
  await page.getByTestId("contact-alias").fill("Bobby");
  await page.getByTestId("contact-note").fill("Met at the meetup");
  await expect(save).toBeEnabled();
  await save.click();
  await authorize(page);
  await expect(page.getByTestId("contact-details-outcome")).toHaveText("Saved");
  await expect(names(page, "contacts")).toContainText(["Bobby"]);
  await expect(page.getByTestId("contact-alias")).toHaveValue("Bobby");
  await expect(save).toBeDisabled();

  await page.getByTestId("contact-hide").check();
  await authorize(page);
  await expect(names(page, "contacts")).not.toContainText(["Bobby"]);
  await group(page, "hidden").getByRole("button").click();
  await expect(names(page, "hidden")).toContainText(["Bobby"]);
  await expect(page.getByTestId("contact-hide")).toBeChecked();
  expect(errors).toEqual([]);
});

test("D7 Hide keeps an alias being typed, and a refused Hide shows the saved value again", async ({ page }) => {
  await openContacts(page);
  await row(page, "Bob Builder").click();
  await page.getByTestId("contact-alias").fill("Bobby");
  await page.getByTestId("contact-hide").check();
  await authorize(page);
  await expect(page.getByTestId("contact-details-outcome")).toHaveText("Saved");
  await expect(page.getByTestId("contact-hide")).toBeChecked();
  // Hide saved the alias as it was; the one typed stays, unsaved.
  await expect(page.getByTestId("contact-alias")).toHaveValue("Bobby");
  await expect(page.getByTestId("contact-details-save")).toBeEnabled();

  await openContacts(page, "dashpay-low-credits");
  await row(page, "Bob Builder").click();
  await page.getByTestId("contact-hide").check();
  await authorize(page);
  await expect(page.getByTestId("contact-details-outcome")).toHaveText("Not enough credits. Top up your credits and try again.");
  await expect(page.getByTestId("contact-hide")).not.toBeChecked();
});

test("AND-093 lists a contact's payments 50 at a time, filters them and opens one", async ({ page }) => {
  await openContacts(page);
  await row(page, "Bob Builder").click();
  const rows = page.getByTestId("contact-activity-row");
  await expect(rows).toHaveCount(50);
  await page.getByTestId("contact-activity-more").click();
  await expect(page.getByTestId("contact-activity-more")).toHaveCount(0);
  const all = await rows.count();
  expect(all).toBeGreaterThan(50);

  const filter = page.getByTestId("contact-activity-filter");
  await filter.getByRole("button", { name: "Sent" }).click();
  await expect(filter.getByRole("button", { name: "Sent" })).toHaveAttribute("aria-pressed", "true");
  await expect(rows.first().locator(".tx-row-title")).toHaveText("Sent");
  await expect(rows.locator(".tx-row-title", { hasText: "Received" })).toHaveCount(0);

  // The wallet's own payment from Bob (the fixture's other rows are made up, not in its history).
  await filter.getByRole("button", { name: "Received" }).click();
  await expect(rows.first().locator(".tx-row-title")).toHaveText("Received");
  const txids = await rows.evaluateAll((els) => els.map((e) => e.getAttribute("data-txid") ?? ""));
  const real = txids.find((t) => !/^(..)\1{31}$/.test(t))!;
  expect(real).toBeTruthy();
  await page.locator(`[data-testid="contact-activity-row"][data-txid="${real}"]`).click();
  await expect(page.getByTestId("tx-detail")).toBeVisible();
  await expect(page.getByTestId("tx-detail-txid")).toContainText(real.slice(0, 8));
});

test("AND-093 a contact without payments says so", async ({ page }) => {
  await openContacts(page);
  await row(page, "Carol").click();
  await expect(page.getByTestId("contact-activity-empty")).toHaveText("No payments with this contact yet.");
});

test("IOS-072 D10 without DashPay keys the banner offers Enable with its fee, and Accept waits for it", async ({ page }) => {
  const errors = await openContacts(page, "dashpay-no-keys");
  const banner = page.getByTestId("contacts-enable-banner");
  await expect(banner).toContainText("Enable DashPay");
  await expect(page.getByTestId("contacts-enable-fee")).toHaveText("61,331");
  await expect(rowItem(page, "heidi").getByTestId("contact-row-accept")).toBeDisabled();
  await row(page, "heidi").click();
  await expect(page.getByTestId("contact-accept")).toBeDisabled();
  await expect(page.getByTestId("contact-pane-note")).toHaveText("Enable DashPay first.");

  await page.getByTestId("contacts-enable").click();
  await authorize(page);
  await expect(page.locator(".toast")).toHaveText("DashPay is enabled.");
  await expect(banner).toHaveCount(0);
  await expect(page.getByTestId("contact-accept")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("D4 a refused accept says why in the pane", async ({ page }) => {
  await openContacts(page, "dashpay-low-credits");
  await row(page, "heidi").click();
  await page.getByTestId("contact-accept").click();
  await authorize(page);
  await expect(page.getByTestId("contact-pane-outcome")).toHaveText("Not enough credits. Top up your credits and try again.");
  await expect(names(page, "requests")).toHaveText(["heidi", "Grace Hopper"]);
});
