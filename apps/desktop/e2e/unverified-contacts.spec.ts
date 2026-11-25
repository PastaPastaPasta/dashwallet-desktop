import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// H-09: contacts whose data is not verified yet, on the fixture backend. A payment to one waits
// in prepare ("Checking contact…"); while the contacts are still being re-checked it gives way to
// a notice with Try again, and when the data doesn't match it is refused; Accept and Send Contact
// Request warn and stay on; the row keeps "Unverified".

const UPDATING = "Dash Wallet hasn't finished checking this contact's payment details, so nothing was sent. You can pay this contact once it has.";
const MISMATCH = "Dash Wallet couldn't confirm this contact's payment details with the Dash network, so nothing was sent. Try again later.";
const ACCEPT = "Dash Wallet can't check this profile yet. You can still accept. Payments to this contact wait until their details are checked.";
const SEND = "Dash Wallet can't check this profile yet. You can still send the request. Payments to this contact wait until their details are checked.";

async function open(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const navigate = (page: Page, route: string) => page.evaluate((r) => window.__dwFixture!.shellCommand({ kind: "navigate", route: r as "pay" }), route);

async function reviewErin(page: Page) {
  await navigate(page, "pay");
  await page.getByTestId("pay-tabs-contact").click();
  await page.getByTestId("pay-picker-row").filter({ has: page.locator(".contact-row-name", { hasText: "Erin" }) }).click();
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-fee-line").waitFor();
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).click();
  await page.getByTestId("pay-confirm").waitFor();
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
}

async function openContacts(page: Page) {
  await navigate(page, "contacts");
  await expect(page.getByTestId("page-contacts").locator("h1")).toBeFocused();
}

const row = (page: Page, name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });

for (const theme of THEMES) {
  test(`H-09 a contact payment waits on "Checking contact…", then the form says the contacts are still updating, with Try again (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-contact-unchecked", theme, "&hold=draft_prepare");
    await reviewErin(page);
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-confirm-send")).toHaveText("Checking contact…");
    await expect(page.getByTestId("pay-confirm-send")).toBeDisabled();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.evaluate(() => window.__dwFixture!.release("draft_prepare"));
    const notice = page.getByTestId("pay-banner").and(page.locator('[data-kind="updating"]'));
    await expect(notice).toHaveAttribute("role", "status");
    await expect(notice.getByTestId("pay-banner-text")).toHaveText(UPDATING);
    await expect(notice.getByTestId("pay-banner-retry")).toHaveText("Try again");
    // A notice, not an error: no result sheet, no alert, the payment still on the form.
    await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
    await expect(page.getByTestId("pay-result")).toHaveCount(0);
    await expect(page.getByRole("alert").filter({ hasText: UPDATING })).toHaveCount(0);
    await expect(page.getByTestId("pay-problem")).toHaveCount(0);
    await expect(page.getByTestId("pay-amount")).toHaveValue("0.01");
    // Focus goes to Try again, so the notice is read.
    await expect(notice.getByTestId("pay-banner-retry")).toBeFocused();
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("H-09 Try again estimates again and reopens the review; Send checks again and still sends nothing to an unchecked contact", async ({ page }) => {
  const errors = await open(page, "dashpay-contact-unchecked");
  await reviewErin(page);
  await page.getByTestId("pay-confirm-send").click();
  const notice = page.getByTestId("pay-banner").and(page.locator('[data-kind="updating"]'));
  await expect(notice).toBeVisible();
  const calls = () => page.evaluate(() => [...window.__dwFixture!.calls]);
  // Once the form's own re-estimate after the sheet closed is in.
  await expect(notice.getByTestId("pay-banner-retry")).toBeEnabled();
  const estimates = (await calls()).filter((c) => c === "draft_estimate").length;
  await notice.getByTestId("pay-banner-retry").click();
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
  await expect(notice).toHaveCount(0);
  expect((await calls()).filter((c) => c === "draft_estimate").length).toBeGreaterThan(estimates);
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(notice.getByTestId("pay-banner-text")).toHaveText(UPDATING);
  await expect(page.getByTestId("pay-result")).toHaveCount(0);
  const after = await calls();
  expect(after.filter((c) => c === "draft_prepare")).toHaveLength(2);
  expect(after).not.toContain("draft_broadcast");
  expect(errors).toEqual([]);
});

test("H-09 a contact payment whose data doesn't match is refused in plain words", async ({ page }) => {
  await open(page, "dashpay-contact-mismatch");
  await reviewErin(page);
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result")).toContainText(MISMATCH);
});

test("H-09 a payment to an address reads Sending… while it is prepared, not Checking contact…", async ({ page }) => {
  await open(page, "dashpay-registered", "light", "&hold=draft_prepare");
  await navigate(page, "pay");
  await page.getByTestId("pay-tabs-address").click();
  await page.getByTestId("pay-address").fill(PAYEE);
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).click();
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-confirm-send")).toHaveText("Sending…");
  await page.evaluate(() => window.__dwFixture!.release("draft_prepare"));
  await expect(page.getByTestId("pay-result")).toContainText("Payment sent");
});

for (const theme of THEMES) {
  test(`H-09 Accept warns on an unverified profile and stays on; the row keeps Unverified (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-unverified-request", theme);
    await openContacts(page);
    await expect(row(page, "Grace Hopper").locator(".chip-warning")).toHaveText("Unverified");
    await row(page, "Grace Hopper").click();
    await expect(page.getByTestId("contact-pane-unverified")).toHaveText(ACCEPT);
    await expect(page.getByTestId("contact-accept")).toBeEnabled();
    expect(await seriousAxeViolations(page)).toEqual([]);
    // A verified requester gets no warning.
    await row(page, "heidi").click();
    await expect(page.getByTestId("contact-accept")).toBeVisible();
    await expect(page.getByTestId("contact-pane-unverified")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`H-09 the bell marks an unverified request and keeps its one-tap Accept (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-unverified-request", theme);
    await page.getByTestId("toolbar-bell").click();
    const grace = page.getByTestId("bell-request").filter({ hasText: "Grace Hopper" });
    await expect(grace.locator(".chip-warning")).toHaveText("Unverified");
    await expect(page.getByTestId("bell-request").filter({ hasText: "heidi" }).locator(".chip-warning")).toHaveCount(0);
    await expect(grace.getByRole("button", { name: /Accept/ })).toBeEnabled();
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`H-09 Send Contact Request warns on an unverified profile and stays on (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-registered", theme);
    await openContacts(page);
    await page.getByTestId("contacts-add").click();
    const field = page.getByRole("searchbox", { name: "Search username" });
    await field.fill("mat");
    await page.getByTestId("add-contact-hit").filter({ has: page.locator(".contact-row-name", { hasText: "mateo" }) }).click();
    await expect(page.getByTestId("add-contact-unverified")).toHaveText(SEND);
    await expect(page.getByTestId("add-contact-send")).toBeEnabled();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await field.fill("mar");
    await page.getByTestId("add-contact-hit").filter({ has: page.locator(".contact-row-name", { hasText: "Marta" }) }).click();
    await expect(page.getByTestId("add-contact-send")).toBeVisible();
    await expect(page.getByTestId("add-contact-unverified")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("H-09 Pay's found user warns before Send Contact Request to an unverified profile", async ({ page }) => {
  await open(page, "dashpay-registered");
  await navigate(page, "pay");
  await page.getByTestId("pay-tabs-contact").click();
  await page.getByTestId("pay-contact-search").locator("input").fill("mateo");
  await page.getByTestId("network-row").filter({ has: page.locator(".contact-row-name", { hasText: "mateo" }) }).click();
  await expect(page.getByTestId("pay-found-unverified")).toHaveText(SEND);
  await expect(page.getByTestId("pay-found-send")).toBeEnabled();
});

test("H-09 Tools ▸ Information says what the trusted-only setting means for contact payments", async ({ page }) => {
  await open(page, "dashpay-info-trusted-only");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "information" }));
  await expect(page.getByTestId("information-dashpay")).toContainText("Dash Wallet's own checks are off. Payments to contacts rely on the Platform service's data without them.");
});

test("H-09 the notice outlives the page: a form shown again after the refusal still says it", async ({ page }) => {
  const errors = await open(page, "dashpay-contact-unchecked", "light", "&hold=draft_prepare");
  await reviewErin(page);
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-confirm-send")).toHaveText("Checking contact…");
  await navigate(page, "home");
  await page.evaluate(() => window.__dwFixture!.release("draft_prepare"));
  await navigate(page, "pay");
  await page.getByTestId("pay-tabs-contact").click();
  await page.getByTestId("pay-picker-row").filter({ has: page.locator(".contact-row-name", { hasText: "Erin" }) }).click();
  const notice = page.getByTestId("pay-banner").and(page.locator('[data-kind="updating"]'));
  await expect(notice.getByTestId("pay-banner-text")).toHaveText(UPDATING);
  await expect(page.getByTestId("pay-result")).toHaveCount(0);
  expect(await page.evaluate(() => window.__dwFixture!.calls)).not.toContain("draft_broadcast");
  expect(errors).toEqual([]);
});
