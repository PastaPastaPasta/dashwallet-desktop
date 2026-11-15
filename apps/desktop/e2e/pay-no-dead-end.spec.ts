import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Pay that never dead-ends (PRODUCT #6): Pay opens on To an address without contacts and on To a
// contact with them; a username that is no contact is searched on DashPay as Add contact does,
// and the user found gets a card; nothing found offers To an address; no username leaves one line.

async function openPay(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-tabs").waitFor();
  return errors;
}

const selected = (page: Page, tab: "contact" | "address") => expect(page.getByTestId(`pay-tabs-${tab}`)).toHaveAttribute("aria-selected", "true");
const searchField = (page: Page) => page.getByTestId("pay-contact-search").locator("input");
const searchFor = (page: Page, text: string) => searchField(page).fill(text);
const networkRow = (page: Page, name: string) => page.getByTestId("network-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });

test("PAY-6 Pay opens on To a contact with contacts, and on To an address without", async ({ page }) => {
  const errors = await openPay(page, "dashpay-registered");
  await selected(page, "contact");
  await page.getByTestId("pay-frequent").waitFor();
  for (const scenario of ["dashpay-contacts-empty", "dashpay-no-identity"]) {
    await openPay(page, scenario);
    await selected(page, "address");
    await expect(page.getByTestId("pay-address")).toBeVisible();
  }
  expect(errors).toEqual([]);
});

test("PAY-6 coming back to Pay chooses its tab again; a tab picked meanwhile stays while Pay is open", async ({ page }) => {
  await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-tabs-contact").click();
  await page.getByTestId("pay-picker-empty").waitFor();
  // Opening Pay while it is open keeps the tab picked.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await selected(page, "contact");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await selected(page, "address");
});

test("PAY-6 without a username To a contact is a single Join DashPay line", async ({ page }) => {
  await openPay(page, "dashpay-no-identity");
  await page.getByTestId("pay-tabs-contact").click();
  const line = page.getByTestId("pay-contact-none");
  await expect(line).toHaveText("Join DashPay to pay people by their username. Join DashPay");
  await expect(page.locator(".empty-state")).toHaveCount(0);
  await page.getByTestId("pay-contact-join").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
});

test("PAY-6 a username that is no contact finds the user, who gets a contact request", async ({ page }) => {
  const errors = await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-tabs-contact").click();
  await expect(page.getByTestId("pay-picker-empty")).toContainText("Type a username above to find someone on DashPay.");
  await searchFor(page, "mar");
  await expect(page.getByTestId("pay-search-announce")).toHaveText(/people found on DashPay|person found on DashPay/);
  await networkRow(page, "Marta").click();
  // The card's heading takes focus.
  await expect(page.getByTestId("pay-found-card").getByRole("heading", { name: "Marta" })).toBeFocused();
  await expect(page.getByTestId("pay-found-line")).toHaveText("You can pay Marta once they accept your contact request.");
  await page.getByTestId("pay-found-send").click();
  await authorize(page);
  // The line says the request went, and takes focus; no second line repeats it.
  await expect(page.getByTestId("pay-found-line")).toHaveText("Contact request sent. You can pay Marta once they accept.");
  await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
  await expect(page.getByTestId("pay-found-status")).toBeFocused();
  await expect(page.getByTestId("pay-found-outcome")).toHaveCount(0);
  // Change goes back to the search as it was, focused; Marta is still found, as a request sent.
  await page.getByTestId("pay-found-back").click();
  await expect(searchField(page)).toHaveValue("mar");
  await expect(searchField(page)).toBeFocused();
  await expect(networkRow(page, "Marta")).toContainText("Request sent");
  await searchFor(page, "marta");
  await networkRow(page, "Marta").click();
  await expect(page.getByTestId("pay-found-line")).toHaveText("Contact request sent. You can pay Marta once they accept.");
  await expect(page.getByTestId("pay-picker-none")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("PAY-6 a request sent, an ignored sender and oneself are found, not 'No one found'", async ({ page }) => {
  const errors = await openPay(page, "dashpay-registered");
  const cases = [
    { text: "fra", name: "Frank", note: "Request sent", line: "Contact request sent. You can pay Frank once they accept." },
    { text: "ivan", name: "Ivan", note: "Ignored", line: "You ignored their contact request." },
    { text: "alice", name: "Alice", note: "You", line: "This is you." },
  ];
  for (const c of cases) {
    await searchFor(page, c.text);
    const row = page.getByTestId("network-row").filter({ hasText: c.note });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText(c.name);
    await expect(page.getByTestId("pay-picker-none")).toHaveCount(0);
    await row.click();
    await expect(page.getByTestId("pay-found-line")).toHaveText(c.line);
    await page.getByTestId("pay-found-back").click();
  }
  expect(errors).toEqual([]);
});

test("PAY-6 a refused Enable on the found user's card says why", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-no-keys", "light", "&fail=dashpay_enable_keys:none"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await searchFor(page, "mal");
  await networkRow(page, "Mallory").click();
  await expect(page.getByTestId("pay-found-line")).toHaveText("Enable DashPay to send contact requests.");
  await page.getByTestId("pay-found-enable").click();
  await authorize(page);
  await expect(page.getByTestId("pay-found-enable-outcome")).toBeVisible();
  await expect(page.getByTestId("pay-found-enable-outcome")).not.toBeEmpty();
  await expect(page.getByTestId("pay-found-enable")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("PAY-6 under two characters without contacts asks to keep typing", async ({ page }) => {
  await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-tabs-contact").click();
  await searchFor(page, "z");
  await expect(page.getByTestId("pay-picker-empty")).toContainText("Keep typing to search DashPay usernames.");
  await searchFor(page, "zz");
  await expect(page.getByTestId("pay-picker-none")).toContainText("No one found");
  await expect(page.getByTestId("pay-search-announce")).toHaveText("No one found");
});

test("PAY-6 a username search that fails is announced", async ({ page }) => {
  await openPay(page, "dashpay-search-offline");
  await searchFor(page, "mal");
  await expect(page.getByTestId("contacts-network-error")).toBeVisible();
  await expect(page.getByTestId("pay-search-announce")).toHaveText("Could not search");
});

test("PAY-6 a wallet switch on Pay chooses the tab again for that wallet", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  await selected(page, "contact");
  await page.getByTestId("pay-tabs-address").click();
  await selected(page, "address");
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
  await selected(page, "contact");
  await page.getByTestId("pay-frequent").waitFor();
});

test("PAY-6 a wallet switch after a payment link keeps the link's form on To an address", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  await selected(page, "contact");
  const address = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
  await page.evaluate((a) => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address: a, amount: null, label: null, message: null }, username: null }]), address);
  await selected(page, "address");
  await expect(page.getByTestId("pay-address")).toHaveValue(address);
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
  await selected(page, "address");
  await expect(page.getByTestId("pay-address")).toHaveValue(address);
});

test("PAY-6 a link's form is gone once Pay is left; a later wallet switch chooses the tab again", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address: "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf", amount: null, label: null, message: null }, username: null }]));
  await selected(page, "address");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await selected(page, "contact");
  await page.getByTestId("pay-tabs-address").click();
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
  await selected(page, "contact");
});

test("PAY-6 a contact picked on Pay is gone once Pay is left; the next visit opens the chooser", async ({ page }) => {
  await openPay(page, "dashpay-registered");
  await page.getByTestId("pay-picker-row").first().click();
  await expect(page.getByTestId("pay-payee-change")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await selected(page, "contact");
  await expect(page.getByTestId("pay-contact-search")).toBeVisible();
  await expect(page.getByTestId("pay-payee-change")).toHaveCount(0);
});

test("PAY-6 a contact or request that matches stays in the picker; strangers show under it", async ({ page }) => {
  // Grace's request is in Contact Requests: the search finds her there, not on the network.
  await openPay(page, "dashpay-registered");
  await searchFor(page, "gra");
  await expect(page.getByTestId("pay-picker-row")).toHaveCount(1);
  await expect(page.getByTestId("network-row")).toHaveCount(0);
  // A stranger whose name shares the prefix is found on the network beside the contacts.
  await searchFor(page, "ma");
  await networkRow(page, "Mallory").click();
  await expect(page.getByTestId("pay-found-send")).toHaveText("Send Contact Request");
});

test("PAY-6 nothing found says so and offers To an address", async ({ page }) => {
  const errors = await openPay(page, "dashpay-registered");
  await searchFor(page, "zzz");
  const none = page.getByTestId("pay-picker-none");
  await expect(none).toContainText("No one found");
  await expect(none).toContainText("No contact or DashPay username starts with “zzz”.");
  await expect(page.getByTestId("pay-search-announce")).toHaveText("No one found");
  await page.getByTestId("pay-none-to-address").click();
  await selected(page, "address");
  expect(errors).toEqual([]);
});

const STATES: Record<string, { scenario: string; go: (page: Page) => Promise<unknown> }> = {
  "no-contacts": { scenario: "dashpay-contacts-empty", go: (page) => page.getByTestId("pay-address").waitFor() },
  "no-username": {
    scenario: "dashpay-no-identity",
    go: async (page) => (await page.getByTestId("pay-tabs-contact").click(), page.getByTestId("pay-contact-none").waitFor()),
  },
  found: {
    scenario: "dashpay-contacts-empty",
    go: async (page) => {
      await page.getByTestId("pay-tabs-contact").click();
      await searchFor(page, "mar");
      await networkRow(page, "Marta").click();
      await page.getByTestId("pay-found-send").waitFor();
    },
  },
  sent: {
    scenario: "dashpay-contacts-empty",
    go: async (page) => {
      await page.getByTestId("pay-tabs-contact").click();
      await searchFor(page, "mar");
      await networkRow(page, "Marta").click();
      await page.getByTestId("pay-found-send").click();
      await authorize(page);
      await expect(page.getByTestId("pay-found-status")).toBeFocused();
    },
  },
  "after-change": {
    scenario: "dashpay-contacts-empty",
    go: async (page) => {
      await page.getByTestId("pay-tabs-contact").click();
      await searchFor(page, "mar");
      await networkRow(page, "Marta").click();
      await page.getByTestId("pay-found-send").click();
      await authorize(page);
      await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
      await page.getByTestId("pay-found-back").click();
      await expect(networkRow(page, "Marta")).toContainText("Request sent");
    },
  },
  "no-match": { scenario: "dashpay-registered", go: async (page) => (await searchFor(page, "zzz"), page.getByTestId("pay-none-to-address").waitFor()) },
};

for (const theme of THEMES) {
  for (const [name, { scenario, go }] of Object.entries(STATES)) {
    test(`pay-no-dead-end ${name} ${theme}: 0 serious axe issues`, async ({ page }, info) => {
      const errors = await openPay(page, scenario, theme);
      await go(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-pay-no-dead-end-${name}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
