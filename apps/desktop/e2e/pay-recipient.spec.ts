import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl } from "./helpers";

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";

async function pay(page: Page) {
  await page.goto(fixtureUrl("dashpay-registered", "light"));
  await page.getByTestId("sidebar-pay").click();
  return page.getByRole("combobox", { name: "Pay to" });
}

test("a pasted payment URI reviews and sends the exact address and amount", async ({ page }) => {
  const field = await pay(page);
  await field.fill(`dash:${ADDRESS}?amount=0.02500001&label=Rent`);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.02500001");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.02500001");
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_broadcast"))).toHaveLength(1);
});

test("keyboard suggestions lead to the contact send, with no layout shift", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  const before = await amount.boundingBox();
  await field.fill("Bob");
  await expect(page.getByRole("option").first()).toContainText("Bob Builder");
  expect(await amount.boundingBox()).toEqual(before);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await field.press("ArrowDown");
  await expect(field).toHaveAttribute("aria-activedescendant", /option/);
  await field.press("Escape");
  await expect(field).toHaveAttribute("aria-expanded", "false");
  await field.press("ArrowDown");
  await field.press("Enter");
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  await amount.fill("0.01");
  await page.getByTestId("pay-note").fill("Lunch");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await expect(page.getByTestId("pay-confirm")).toContainText("Lunch");
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_broadcast"))).toHaveLength(1);
});

test("Contacts opens the list and clearing a picked contact returns to the same field", async ({ page }) => {
  const field = await pay(page);
  await page.getByRole("button", { name: "Contacts", exact: true }).click();
  await expect(page.getByRole("listbox")).toBeVisible();
  await page.getByRole("option", { name: /Carol/ }).click();
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Carol");
  await page.getByTestId("pay-payee-change").click();
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("");
  await expect(page.getByTestId("pay-address-book")).toBeVisible();
});

test("pasting a QR image resolves in the same field", async ({ page }) => {
  const field = await pay(page);
  await field.evaluate((el, address) => {
    const data = new DataTransfer();
    data.items.add(new File([`fixture-qr:\ndash:${address}?amount=0.25`], "fixture.png", { type: "image/png" }));
    el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, ADDRESS);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(field).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.25000000");
});

test("changing a quoted recipient requires a fresh estimate before review", async ({ page }) => {
  const field = await pay(page);
  const review = page.getByTestId("pay-continue");
  await field.fill(ADDRESS);
  await page.getByTestId("pay-amount").fill("0.02");
  await expect(review).toBeEnabled();
  await page.evaluate(() => {
    (window as Window & { releaseEstimate?: () => void }).releaseEstimate = window.__dwFixture!.hold("draft_estimate");
  });

  const next = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await field.fill(next);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(review).toBeDisabled();
  await expect(page.getByTestId("pay-fee-line")).toHaveCount(0);
  await page.getByTestId("pay-amount").press("Enter");
  await expect(page.getByTestId("pay-confirm")).toHaveCount(0);

  await page.evaluate(() => (window as Window & { releaseEstimate?: () => void }).releaseEstimate?.());
  await review.click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(next);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.02");
});

test("a late payment link cannot replace a contact selected while it was resolving", async ({ page }) => {
  const field = await pay(page);
  await page.evaluate(() => {
    (window as Window & { releaseLink?: () => void }).releaseLink = window.__dwFixture!.hold("route_link");
  });
  await field.fill(`dash:${ADDRESS}?amount=0.25&label=Rent`);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("route_link"))).toBe(true);
  await field.fill("Bob");
  await page.getByRole("option", { name: /Bob Builder/ }).click();
  await page.evaluate(() => (window as Window & { releaseLink?: () => void }).releaseLink?.());

  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  const amount = page.getByTestId("pay-amount");
  await expect(amount).toHaveValue("");
  await amount.fill("0.01");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.01");
});

test("an invalid address reports the existing inline error without moving the form", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  await expect(amount).toBeVisible();
  const before = await amount.boundingBox();
  await field.fill(ADDRESS.slice(0, -1) + "0");
  await expect(field).toHaveAccessibleDescription("The recipient address is not valid. Please recheck.");
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  expect(await amount.boundingBox()).toEqual(before);
});

test("a DashPay user link resolves to the verified contact identity", async ({ page }) => {
  const field = await pay(page);
  const identity = await page.evaluate(() =>
    window.__dwFixture!.world.dashpay.contacts.find((c) => c.summary.username === "bob")!.summary.contact,
  );
  await field.fill(`dashpay://user?id=${identity}&username=bob`);
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("@bob");
  const calls = await page.evaluate(() => window.__dwFixture!.calls);
  expect(calls).toContain("dashpay_verify_scanned");
  expect(calls).toContain("draft_set_contact_recipient");
  expect(calls).not.toContain("draft_set_recipients");
});
