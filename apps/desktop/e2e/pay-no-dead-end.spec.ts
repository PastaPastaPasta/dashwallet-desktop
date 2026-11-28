import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

async function openPay(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-address").waitFor();
  return errors;
}

const searchField = (page: Page) => page.getByTestId("pay-address");
const searchFor = (page: Page, text: string) => searchField(page).fill(text);
/** The engine's signal that DashPay's state changed: every DashPay read is read again. */
const dashpayChanged = (page: Page) =>
  page.evaluate(() => {
    const w = window.__dwFixture!.world;
    window.__dwFixture!.emit({ kind: "platform", network: w.launch.network.dirName, walletId: w.wallets[0].summary.walletId, change: { domain: "names" } } as never);
  });
const networkRow = (page: Page, name: string) => page.getByTestId("network-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });

test("Pay opens with the recipient field in every DashPay state", async ({ page }) => {
  for (const scenario of ["dashpay-registered", "dashpay-contacts-empty", "dashpay-no-identity"]) {
    const errors = await openPay(page, scenario);
    await expect(searchField(page)).toBeEditable();
    await expect(searchField(page)).toHaveValue("");
    await expect(page.getByTestId("pay-recipient-popover")).toHaveCount(0);
    expect(errors).toEqual([]);
  }
});

test("The Contacts button offers Join DashPay without a username", async ({ page }) => {
  await openPay(page, "dashpay-no-identity");
  await page.getByTestId("pay-contacts").click();
  const line = page.getByTestId("pay-contact-none");
  await expect(line).toContainText("Join DashPay to pay people by their username.");
  await expect(page.locator(".empty-state")).toHaveCount(0);
  await page.getByTestId("pay-contact-join").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
});

test("PAY-6 a username that is no contact finds the user, who gets a contact request", async ({ page }) => {
  const errors = await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-contacts").click();
  await searchFor(page, "mar");
  await expect(page.getByTestId("pay-search-announce")).toHaveText(/people found on DashPay|person found on DashPay/);
  await networkRow(page, "Marta").click();
  // The card's heading takes focus.
  await expect(page.getByTestId("pay-found-card").getByRole("heading", { name: "Marta" })).toBeFocused();
  await expect(page.getByTestId("pay-found-line")).toHaveText("You can pay Marta once they accept your contact request.");
  // The eligibility reads after the send fail, which keeps the last answer: the card says the
  // request went without them.
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_eligibility", "none"));
  await page.getByTestId("pay-found-send").click();
  await authorize(page);
  // The line says the request went, and takes focus; no second line repeats it.
  await expect(page.getByTestId("pay-found-line")).toHaveText("Contact request sent. You can pay Marta once they accept.");
  await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_eligibility", null));
  await expect(page.getByTestId("pay-found-status")).toBeFocused();
  await expect(page.getByTestId("pay-found-outcome")).toBeEmpty();
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

test("PAY-6 a found user's check that fails on a re-read offers Try again, not the answer before it", async ({ page }) => {
  const errors = await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-contacts").click();
  await searchFor(page, "mar");
  await networkRow(page, "Marta").click();
  await expect(page.getByTestId("pay-found-send")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_eligibility", "rpc"));
  await dashpayChanged(page);
  const card = page.getByTestId("pay-found-status");
  await expect(card.getByRole("button", { name: "Try again" })).toBeVisible();
  await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_eligibility", null));
  await card.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("pay-found-send")).toBeVisible();
  expect(errors).toEqual([]);
});

/** Makes the next call of `command` fail, and holds it until `release(page)`. */
const failHeld = (page: Page, command: "dashpay_status" | "dashpay_contacts") =>
  page.evaluate((command) => {
    const f = window.__dwFixture!;
    f.fail(command, "rpc");
    (window as unknown as { __release: () => void }).__release = f.hold(command);
  }, command);
const release = (page: Page) => page.evaluate(() => (window as unknown as { __release: () => void }).__release());

for (const command of ["dashpay_status", "dashpay_contacts"] as const) {
  test(`A pending or failed ${command} read does not block address entry`, async ({ page }) => {
    await openPay(page, "dashpay-registered");
    const address = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
    await failHeld(page, command);
    if (command === "dashpay_contacts") await page.getByTestId("pay-contacts").click();
    else await dashpayChanged(page);
    await searchField(page).fill(address);
    await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
    await release(page);
    await expect(searchField(page)).toHaveValue(address);
  });
}

test("PAY-6 a request sent from the card turns into Pay once they accept", async ({ page }) => {
  const errors = await openPay(page, "dashpay-contacts-empty");
  await page.getByTestId("pay-contacts").click();
  await searchFor(page, "mar");
  await networkRow(page, "Marta").click();
  await page.getByTestId("pay-found-send").click();
  await authorize(page);
  await expect(page.getByTestId("pay-found-line")).toHaveText("Contact request sent. You can pay Marta once they accept.");
  await expect(page.getByTestId("pay-found-pay")).toHaveCount(0);
  // Marta accepts on her device.
  await page.evaluate(() => {
    const w = window.__dwFixture!.world;
    const marta = w.dashpay.contacts.find((c) => c.summary.username === "marta")!;
    marta.summary.relation = "contact";
  });
  await dashpayChanged(page);
  await expect(page.getByTestId("pay-found-pay")).toHaveText("Pay Marta");
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
  await page.getByTestId("pay-contacts").click();
  await searchFor(page, "z");
  await expect(page.getByTestId("pay-picker-none")).toContainText("Keep typing to search DashPay usernames.");
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

test("Switching wallets clears an ordinary recipient draft", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  await searchFor(page, "bob");
  await expect(page.getByTestId("pay-picker-row").first()).toBeVisible();
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
  await expect(searchField(page)).toHaveValue("");
  await expect(page.getByTestId("pay-recipient-popover")).toHaveCount(0);
});

test("PAY-6 a wallet switch after a payment link keeps the linked address", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  const address = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
  await page.evaluate((a) => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address: a, amount: null, label: null, message: null }, username: null }]), address);
  await expect(page.getByTestId("pay-address")).toHaveValue(address);
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
  await expect(page.getByTestId("pay-address")).toHaveValue(address);
});

test("PAY-6 a link's form is gone once Pay is left; wallet switches keep the form empty", async ({ page }) => {
  await openPay(page, "dashpay-two-wallets");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address: "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf", amount: null, label: null, message: null }, username: null }]));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await page.getByTestId("toolbar-wallet-picker").selectOption({ index: 1 });
});

test("PAY-6 a contact picked on Pay is gone once Pay is left; the next visit has an empty field", async ({ page }) => {
  await openPay(page, "dashpay-registered");
  await page.getByTestId("pay-contacts").click();
  await page.getByTestId("pay-picker-row").first().click();
  await expect(page.getByTestId("pay-payee-change")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await expect(page.getByTestId("pay-address")).toBeVisible();
  await expect(page.getByTestId("pay-payee-change")).toHaveCount(0);
});

test("PAY-6 a payment link that arrives while Pay is open drops the contact picked", async ({ page }) => {
  await openPay(page, "dashpay-registered");
  await page.getByTestId("pay-contacts").click();
  await page.getByTestId("pay-picker-row").first().click();
  await expect(page.getByTestId("pay-payee-change")).toBeVisible();
  await page.evaluate(() =>
    window.__dwFixture!.deliverLinks([
      { kind: "pay", uri: { address: "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf", amount: null, label: null, message: null }, username: null },
    ]),
  );
  await page.getByTestId("pay-contacts").click();
  await expect(page.getByTestId("pay-address")).toBeVisible();
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

test("PAY-6 the username search still answers when the picker's own contacts cannot be read", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-registered", "light", "&fail=dashpay_contacts:none"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-contacts").click();
  await searchFor(page, "ma");
  await expect(networkRow(page, "Mallory")).toBeVisible();
  await expect(page.getByTestId("pay-search-announce")).toContainText("found on DashPay");
});

test("A search miss leaves the same field ready for an address", async ({ page }) => {
  const errors = await openPay(page, "dashpay-registered");
  await searchFor(page, "zzz");
  const none = page.getByTestId("pay-picker-none");
  await expect(none).toContainText("No one found");
  await expect(page.getByTestId("pay-search-announce")).toHaveText("No one found");
  await searchFor(page, "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  expect(errors).toEqual([]);
});

const STATES: Record<string, { scenario: string; go: (page: Page) => Promise<unknown> }> = {
  "no-contacts": { scenario: "dashpay-contacts-empty", go: (page) => page.getByTestId("pay-address").waitFor() },
  "no-username": {
    scenario: "dashpay-no-identity",
    go: async (page) => (await page.getByTestId("pay-contacts").click(), page.getByTestId("pay-contact-none").waitFor()),
  },
  found: {
    scenario: "dashpay-contacts-empty",
    go: async (page) => {
      await page.getByTestId("pay-contacts").click();
      await searchFor(page, "mar");
      await networkRow(page, "Marta").click();
      await page.getByTestId("pay-found-send").waitFor();
    },
  },
  sent: {
    scenario: "dashpay-contacts-empty",
    go: async (page) => {
      await page.getByTestId("pay-contacts").click();
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
      await page.getByTestId("pay-contacts").click();
      await searchFor(page, "mar");
      await networkRow(page, "Marta").click();
      await page.getByTestId("pay-found-send").click();
      await authorize(page);
      await expect(page.getByTestId("pay-found-send")).toHaveCount(0);
      await page.getByTestId("pay-found-back").click();
      await expect(networkRow(page, "Marta")).toContainText("Request sent");
    },
  },
  "no-match": { scenario: "dashpay-registered", go: async (page) => (await searchFor(page, "zzz"), page.getByTestId("pay-picker-none").waitFor()) },
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
