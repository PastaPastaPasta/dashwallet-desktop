import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP3-07: history rows with the contact's avatar and name (DP3-02's attribution), the transaction
// detail naming the contact and opening its pane, and Home's frequent-contacts strip, on the
// fixture's `dashpay-history-contacts` scenario (D10): Marta (no relation to the main identity),
// "Coffee refund" from Bob (a label over the contact), Bob, Dave (`@dave`) and a payment whose
// names have not landed. Tests are named by parity ID: IOS-029 (contact icons on rows), IOS-074
// (the contact's profile).

async function open(page: Page, scenario = "dashpay-history-contacts", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  return errors;
}

const row = (page: Page, title: string) => page.locator(".tx-row").filter({ has: page.locator(".tx-row-title", { hasText: new RegExp(`^${title}$`) }) });

async function activity(page: Page) {
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
}

async function detailOf(page: Page, title: string) {
  await row(page, title).first().click();
  await page.getByTestId("tx-detail-facts").waitFor();
  return page.getByTestId("tx-detail-contact");
}

test("IOS-029 a contact payment's row leads with the contact's avatar; other rows keep the type icon", async ({ page }) => {
  const errors = await open(page);
  const rows = page.getByTestId("page-home").locator(".tx-row");
  await expect(row(page, "Marta").locator(".contact-avatar")).toHaveText("M");
  // The label stays the title; the avatar still says who (D1).
  await expect(row(page, "Coffee refund").locator(".contact-avatar")).toHaveText("BB");
  await expect(row(page, "Bob Builder").first().locator(".contact-avatar")).toHaveText("BB");
  await expect(row(page, "Alice").locator(".contact-avatar")).toHaveCount(0);
  await expect(row(page, "Alice").locator(".tx-row-icon")).toBeVisible();
  // The avatar is decoration: the row's name is its title, not the letters twice.
  await expect(row(page, "Marta")).toHaveAccessibleName(/^Marta /);
  await expect(rows).toHaveCount(5);
  await activity(page);
  await expect(row(page, "@dave").locator(".contact-avatar")).toHaveText("D");
  // A counterparty whose names have not landed keeps the type title and icon (D1).
  const received = row(page, "Received").filter({ has: page.locator(".tx-row-icon") });
  expect(await received.count()).toBeGreaterThan(0);
  await expect(page.getByTestId("tx-list").locator(".contact-avatar")).toHaveCount(4);
  expect(errors).toEqual([]);
});

test("IOS-029 the table names the contact in Address / Label, else the address (D3, D12)", async ({ page }) => {
  await open(page);
  await activity(page);
  await page.getByTestId("tx-mode-table").click();
  const party = page.getByTestId("tx-table").locator(".tx-table-party");
  await expect(party.first()).toHaveText("Marta");
  await expect(party.nth(1)).toHaveText("Coffee refund");
  await expect(party.filter({ hasText: /^@dave$/ })).toHaveCount(1);
  // An unattributed, unlabelled payment shows its address.
  await expect(party.filter({ hasText: /^y[1-9A-HJ-NP-Za-km-z]{33}$/ }).first()).toBeVisible();
  await expect(page.getByTestId("tx-table").locator(".contact-avatar")).toHaveCount(0);
});

test("IOS-074 the detail names the contact and View contact opens its pane", async ({ page }) => {
  const errors = await open(page);
  const fact = await detailOf(page, "Coffee refund");
  await expect(fact.locator(".detail-label")).toHaveText("Received from contact");
  await expect(fact.locator(".contact-avatar")).toHaveText("BB");
  await expect(fact.locator(".tx-detail-contact-name")).toHaveText("Bob Builder");
  await expect(fact.locator(".contact-row-username")).toHaveText("@bob");
  // The address stays under the contact (D3).
  await expect(page.getByTestId("tx-detail-facts")).toContainText("Received with");
  await fact.getByTestId("tx-detail-view-contact").click();
  await expect(page.getByTestId("tx-detail")).toHaveCount(0);
  await expect(page.getByTestId("page-contacts")).toBeVisible();
  await expect(page.getByTestId("contact-detail").getByRole("heading", { name: "Bob Builder" })).toBeVisible();
  const selected = page.getByTestId("contacts-list").locator('[aria-current="true"]');
  await expect(selected).toContainText("Bob Builder");
  // The row that opened the sheet went with Home: focus lands on the contact's row.
  await expect(selected).toBeFocused();
  expect(errors).toEqual([]);
});

test("IOS-074 a username-only contact shows one name line; a stranger has no View contact (D4, D6)", async ({ page }) => {
  await open(page);
  await activity(page);
  let fact = await detailOf(page, "@dave");
  await expect(fact.locator(".detail-label")).toHaveText("Sent to contact");
  await expect(fact.locator(".tx-detail-contact-name")).toHaveText("@dave");
  await expect(fact.locator(".contact-row-username")).toHaveCount(0);
  await expect(fact.getByTestId("tx-detail-view-contact")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("tx-detail")).toHaveCount(0);
  fact = await detailOf(page, "Marta");
  await expect(fact.locator(".tx-detail-contact-name")).toHaveText("Marta");
  // Dave's View contact above came from the same contacts read, so its answer is here already.
  await expect(fact.getByTestId("tx-detail-view-contact")).toHaveCount(0);
});

test("IOS-029 a row whose counterparty has no name yet shows no contact in the detail", async ({ page }) => {
  await open(page);
  await activity(page);
  const nameless = await page.evaluate(() => window.__dwFixture!.world.wallets[0].rows.find((r) => r.counterparty && !r.counterparty.username)!.txid);
  await page.locator(`.tx-row[data-txid="${nameless}"]`).click();
  await page.getByTestId("tx-detail-facts").waitFor();
  await expect(page.getByTestId("tx-detail-contact")).toHaveCount(0);
  await expect(page.locator(".tx-detail-title")).toHaveText("Received");
});

test("DP3-07 Home's frequent strip opens Pay ▸ To a contact on the pick (D7)", async ({ page }) => {
  const errors = await open(page);
  const strip = page.getByTestId("home-frequent");
  await expect(strip.getByRole("heading", { name: "Frequent contacts" })).toBeVisible();
  await expect(strip.getByTestId("home-frequent-chip").locator(".pay-frequent-name")).toHaveText(["Bob Builder", "dave"]);
  await strip.getByTestId("home-frequent-chip").first().click();
  await expect(page.getByTestId("pay-tabs-contact")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  // Pay's own strip is the same component.
  await page.getByTestId("pay-payee").getByRole("button", { name: "Change" }).click();
  await expect(page.getByTestId("pay-frequent-chip").locator(".pay-frequent-name")).toHaveText(["Bob Builder", "dave"]);
  expect(errors).toEqual([]);
});

test("DP3-07 discreet mode hides the strip with the history (D8); no identity, no strip", async ({ page }) => {
  await open(page);
  await expect(page.getByTestId("home-frequent")).toBeVisible();
  await page.getByTestId("hero-amount").click();
  await expect(page.getByTestId("history-hidden")).toBeVisible();
  await expect(page.getByTestId("home-frequent")).toHaveCount(0);
  await open(page, "funded");
  await page.getByTestId("overview-group-0").waitFor();
  await expect(page.getByTestId("home-frequent")).toHaveCount(0);
});

for (const theme of THEMES) {
  test(`Home with the strip and contact rows ${theme}: 0 serious axe issues`, async ({ page }, info) => {
    const errors = await open(page, "dashpay-history-contacts", theme);
    await page.getByTestId("home-frequent-chip").first().waitFor();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-home-contacts.png`), fullPage: true });
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`detail with a contact ${theme}: 0 serious axe issues`, async ({ page }, info) => {
    const errors = await open(page, "dashpay-history-contacts", theme);
    const fact = await detailOf(page, "Coffee refund");
    await fact.getByTestId("tx-detail-view-contact").waitFor();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-tx-detail-contact.png`) });
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`Activity with contact rows ${theme}: 0 serious axe issues`, async ({ page }) => {
    await open(page, "dashpay-history-contacts", theme);
    await activity(page);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });
}
