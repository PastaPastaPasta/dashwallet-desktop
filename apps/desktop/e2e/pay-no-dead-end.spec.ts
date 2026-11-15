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
const searchFor = (page: Page, text: string) => page.getByTestId("pay-contact-search").locator("input").fill(text);
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
  await networkRow(page, "Marta").click();
  await expect(page.getByTestId("pay-found-name")).toHaveText("Marta");
  await expect(page.getByTestId("pay-found-line")).toHaveText("You can pay Marta once they accept your contact request.");
  await page.getByTestId("pay-found-send").click();
  await authorize(page);
  await expect(page.getByTestId("pay-found-outcome")).toHaveText("Contact request sent to Marta.");
  await expect(page.getByTestId("pay-found-line")).toHaveText("Contact request sent. You can pay Marta once they accept.");
  await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
  // Change goes back to the search as it was.
  await page.getByTestId("pay-found-back").click();
  await expect(page.getByTestId("pay-contact-search").locator("input")).toHaveValue("mar");
  expect(errors).toEqual([]);
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
