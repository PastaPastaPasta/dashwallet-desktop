import { expect, test, type Page } from "@playwright/test";
import type { LinkRoute } from "../src/api/generated/LinkRoute";
import { IDENTITY } from "../src/fixture/dashpay";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP2-07: the Contacts split view on the fixture backend, whose `dashpay_contacts` sorts and
// filters as the engine does (src/fixture/contacts.test.ts). Tests are named by parity ID:
// IOS-072 (the Contacts tab: groups, search, hidden, the network search, the setup hint) and
// AND-090 (Android's sort orders).

async function openContacts(page: Page, scenario = "dashpay-registered", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  // A menu or link focuses the page's title in the next frame (shellBridge `focusPage`); a test
  // that focuses a row first can lose it to that frame.
  await expect(page.getByTestId("page-contacts").locator("h1")).toBeFocused();
  return errors;
}

const group = (page: Page, section: string) => page.getByTestId(`contacts-group-${section}`);
const names = (page: Page, section: string) => group(page, section).locator(".contact-row-name");
const row = (page: Page, name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
const search = (page: Page) => page.getByRole("searchbox", { name: "Search for a contact" });
const deliver = (page: Page, routes: LinkRoute[]) => page.evaluate((r) => window.__dwFixture!.deliverLinks(r), routes);

test("IOS-072 groups requests, contacts, pending and a closed Hidden, with the rows' names, chips and notes", async ({ page }) => {
  const errors = await openContacts(page);
  await expect(page.getByTestId("page-contacts").locator("h2.contacts-group-title")).toHaveText([
    "Contact Requests (2)",
    "My Contacts",
    "Pending Requests",
    "Hidden (2)",
  ]);
  await expect(names(page, "requests")).toHaveText(["heidi", "Grace Hopper"]);
  await expect(names(page, "contacts")).toHaveText(["Bob Builder", "Carol", "dave", "Erin"]);
  await expect(names(page, "pending")).toHaveText(["Frank"]);
  await expect(group(page, "requests").locator(".contact-row-note")).toHaveText(["Request received", "Request received"]);
  await expect(group(page, "pending").locator(".contact-row-note")).toHaveText(["Request sent"]);
  await expect(row(page, "Carol").getByTestId("contact-chip-settingUp")).toHaveText("Setting up");
  await expect(row(page, "dave").getByTestId("contact-chip-unavailable")).toHaveText("Payments unavailable");
  await expect(row(page, "Erin").getByTestId("contact-chip-unverified")).toHaveText("Unverified");
  await expect(row(page, "Bob Builder").locator(".contact-row-username")).toHaveText("@bob");
  await expect(row(page, "Bob Builder").locator(".contact-avatar")).toHaveText("BB");

  // Hidden is a closed disclosure; opening it shows ignored senders as such (D5).
  const hidden = group(page, "hidden").getByRole("button", { name: "Hidden (2)" });
  await expect(hidden).toHaveAttribute("aria-expanded", "false");
  await expect(names(page, "hidden")).toHaveCount(0);
  await hidden.click();
  await expect(hidden).toHaveAttribute("aria-expanded", "true");
  await expect(names(page, "hidden")).toHaveText(["Ivan", "Judy (work)"]);
  await expect(row(page, "Ivan").locator(".contact-row-note")).toHaveText("Ignored");
  expect(errors).toEqual([]);
});

test("AND-090 sorts My Contacts and Hidden by the chosen order, for the session; requests stay newest first", async ({ page }) => {
  await openContacts(page);
  const sort = page.getByTestId("contacts-sort");
  await expect(sort).toHaveValue("displayName");
  await expect(sort.locator("option")).toHaveText(["Name", "Username", "Date added", "Last activity"]);
  await sort.selectOption("dateAdded");
  await expect(names(page, "contacts")).toHaveText(["Carol", "Erin", "Bob Builder", "dave"]);
  // The last payment, else the contact's own time.
  await sort.selectOption("lastActivity");
  await expect(names(page, "contacts")).toHaveText(["Bob Builder", "Carol", "Erin", "dave"]);
  // Requests stay newest first whatever the sort: Heidi's came after Grace's, the reverse of their
  // name, username and id orders.
  for (const order of ["displayName", "username", "dateAdded", "lastActivity"]) {
    await sort.selectOption(order);
    await expect(names(page, "requests")).toHaveText(["heidi", "Grace Hopper"]);
  }
  // The choice outlives the page.
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("sidebar-contacts").click();
  await expect(page.getByTestId("contacts-sort")).toHaveValue("lastActivity");
  await expect(names(page, "contacts")).toHaveText(["Bob Builder", "Carol", "Erin", "dave"]);
});

test("IOS-072 filters the list as the user types, opens Hidden for matches, says when nothing matches, and Escape clears", async ({ page }) => {
  await openContacts(page);
  await search(page).fill("builder");
  await expect(page.getByTestId("contact-row")).toHaveCount(1);
  await expect(names(page, "contacts")).toHaveText(["Bob Builder"]);

  // An alias match in Hidden shows it, open, with no disclosure to close it meanwhile.
  await search(page).fill("work");
  await expect(names(page, "hidden")).toHaveText(["Judy (work)"]);
  await expect(group(page, "hidden").getByRole("button", { name: /Hidden/ })).toHaveCount(0);

  // The first four characters of an identity id match it.
  await search(page).fill(IDENTITY.dave.slice(0, 4));
  await expect(names(page, "contacts")).toHaveText(["dave"]);

  await search(page).fill("zzz");
  await expect(page.getByTestId("contacts-no-match")).toContainText("There are no contacts that match “zzz”");

  await search(page).press("Escape");
  await expect(search(page)).toHaveValue("");
  await expect(page.getByTestId("contact-row")).toHaveCount(7);
  // Hidden is closed again: the search opened it, not the user.
  await expect(group(page, "hidden").getByRole("button", { name: "Hidden (2)" })).toHaveAttribute("aria-expanded", "false");
});

test("IOS-072 searches the network from two characters: three new usernames, Show more and a pick open Add contact", async ({ page }) => {
  await openContacts(page);
  await search(page).fill("m");
  await expect(page.getByTestId("contacts-no-match")).toBeVisible();
  await expect(page.getByTestId("contacts-network")).toHaveCount(0);

  await search(page).fill("ma");
  const network = page.getByTestId("contacts-network");
  await expect(network.getByRole("heading")).toHaveText("On the Dash network");
  await expect(network.getByTestId("network-row").locator(".contact-row-name")).toHaveText(["Mallory", "Marta", "mateo"]);
  await expect(network.getByTestId("network-row").nth(2).locator(".chip")).toHaveText("Unverified");

  // Add Contact (DP2-08) takes the search, or the row's hit.
  await page.getByTestId("contacts-network-more").click();
  await expect(page.getByRole("searchbox", { name: "Search username" })).toHaveValue("ma");

  await network.getByTestId("network-row").filter({ hasText: "Marta" }).click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Marta");
  await expect(page.getByTestId("add-contact")).toContainText(IDENTITY.marta);

  // Clearing the field drops the network rows at once.
  await search(page).fill("");
  await expect(network).toHaveCount(0);
});

test("IOS-072 network search shows strangers only: the identity itself and its contacts are listed above", async ({ page }) => {
  await openContacts(page);
  // "bo" finds @bob, who is a contact: the network section has nobody new to show.
  await search(page).fill("bo");
  await expect(names(page, "contacts")).toHaveText(["Bob Builder"]);
  await expect(page.getByTestId("contacts-network-none")).toHaveText("No usernames found");
  await search(page).fill("al");
  await expect(page.getByTestId("contacts-network-none")).toBeVisible();
});

test("IOS-072 a failed network search says so and offers Try again", async ({ page }) => {
  await openContacts(page, "dashpay-search-offline");
  await search(page).fill("ma");
  const error = page.getByTestId("contacts-network-error");
  await expect(error).toContainText("Could not search the Dash network.");
  await expect(error.getByRole("button", { name: "Try again" })).toBeVisible();
  // The local list still answers.
  await expect(page.getByTestId("contacts-no-match")).toBeVisible();
  // DAPI answers again: Try again shows the search running, then the hits.
  await page.evaluate(() => {
    window.__dwFixture!.world.dashpay.searchOffline = false;
  });
  await error.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("network-row")).toHaveCount(3);
  await expect(error).toHaveCount(0);
});

test("IOS-072 the setup hint: finishing while unlocked, Unlock when unlocked for mixing only", async ({ page }) => {
  await openContacts(page);
  const hint = page.getByTestId("contacts-setup-hint");
  await expect(hint).toHaveText("Finishing the setup of 1 contact…");
  await expect(page.getByTestId("contacts-setup-unlock")).toHaveCount(0);

  await openContacts(page, "dashpay-contacts-mixing-only");
  await expect(hint).toContainText("Unlock to finish setting up 1 contact");
  await page.getByTestId("contacts-setup-unlock").click();
  await expect(page.getByTestId("unlock-sheet").first()).toBeVisible();
});

test("selecting a row shows it in the detail slot; a search that filters it out keeps it there", async ({ page }) => {
  await openContacts(page);
  await expect(page.getByTestId("contact-pane-empty")).toHaveText("Select a contact to see the details.");
  await row(page, "Carol").click();
  await expect(row(page, "Carol")).toHaveAttribute("aria-current", "true");
  const detail = page.getByTestId("contact-detail");
  await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Carol");
  await expect(page.getByTestId("contact-detail-identity")).toHaveText(IDENTITY.carol);
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Contact");
  await expect(page.getByTestId("contact-detail-channel")).toHaveText("Setting up");
  // DP2-09 fills the rest of the slot: Carol's payments are still being set up; a payment
  // finishes that (DP3-06 D15).
  await expect(page.getByTestId("contact-pay")).toBeEnabled();
  await expect(page.getByTestId("contact-pane-note")).toHaveText("Payments with this contact are still being set up. The payment finishes the setup.");

  await search(page).fill("grace");
  await expect(names(page, "requests")).toHaveText(["Grace Hopper"]);
  await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Carol");

  // A request row has no channel line.
  await row(page, "Grace Hopper").click();
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Request received");
  await expect(page.getByTestId("contact-detail-channel")).toHaveCount(0);
});

test("the keyboard moves between rows across groups and Enter selects", async ({ page }) => {
  await openContacts(page);
  await row(page, "heidi").focus();
  await page.keyboard.press("ArrowDown");
  await expect(row(page, "Grace Hopper")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(row(page, "Bob Builder")).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(row(page, "Grace Hopper")).toBeFocused();
  // Hidden is closed, so End stops at Pending's last row.
  await page.keyboard.press("End");
  await expect(row(page, "Frank")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(row(page, "Frank")).toBeFocused();
  await page.keyboard.press("Home");
  await expect(row(page, "heidi")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(row(page, "Grace Hopper")).toHaveAttribute("aria-current", "true");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press(" ");
  await expect(row(page, "Bob Builder")).toHaveAttribute("aria-current", "true");
});

test("D11 a selection the search hides leaves the slot when its relation goes", async ({ page }) => {
  const errors = await openContacts(page);
  const detail = page.getByTestId("contact-detail");
  // Carol's relation goes while a search hides her row: the slot empties.
  await row(page, "Carol").click();
  await search(page).fill("grace");
  await expect(names(page, "requests")).toHaveText(["Grace Hopper"]);
  await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Carol");
  await page.evaluate((carol) => {
    const f = window.__dwFixture!;
    f.world.dashpay.contacts = f.world.dashpay.contacts.filter((c) => c.summary.contact !== carol);
    const walletId = f.world.wallets[0].summary.walletId;
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId, change: { domain: "contacts", identity: f.world.dashpay.identities[0].identity } });
  }, IDENTITY.carol);
  await expect(page.getByTestId("contact-pane-empty")).toBeVisible();
  expect(errors).toEqual([]);
});

test("D11 a selection the search hides leaves the slot when another identity becomes main", async ({ page }) => {
  const errors = await openContacts(page);
  const detail = page.getByTestId("contact-detail");
  // Its contacts are not the old identity's.
  await row(page, "Bob Builder").click();
  await search(page).fill("grace");
  await expect(detail.getByRole("heading", { level: 2 })).toHaveText("Bob Builder");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const d = f.world.dashpay;
    const other = "9qWZ5vJxnxV4gNTjYzLM7rpyV8Ff2T3vV9aH6uDWkQZt";
    d.identities = [...d.identities.map((i) => ({ ...i, isMain: false })), { ...d.identities[0], identity: other, index: 1, isMain: true, names: [], mainName: null }];
    d.status = { kind: "ready", main: other };
    d.contacts = [];
    const walletId = f.world.wallets[0].summary.walletId;
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId, change: { domain: "identities" } });
  });
  await expect(page.getByTestId("contacts-no-match")).toBeVisible();
  await expect(page.getByTestId("contact-pane-empty")).toBeVisible();
  await expect(detail).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("IOS-072 a network hit that becomes a request leaves On the Dash network (D21)", async ({ page }) => {
  const errors = await openContacts(page);
  await search(page).fill("ma");
  const mallory = page.locator(`[data-testid="network-row"][data-identity="${IDENTITY.mallory}"]`);
  await expect(mallory).toHaveCount(1);
  // Mallory's request arrives; the cached search answer still calls Mallory a stranger.
  await page.evaluate((id) => {
    const f = window.__dwFixture!;
    const d = f.world.dashpay;
    const request = structuredClone(d.contacts.find((c) => c.summary.relation === "pendingIncoming")!);
    request.summary = { ...request.summary, contact: id, username: "mallory", displayName: "Mallory", alias: null };
    d.contacts = [...d.contacts, request];
    const walletId = f.world.wallets[0].summary.walletId;
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId, change: { domain: "contacts", identity: d.identities[0].identity } });
  }, IDENTITY.mallory);
  await expect(names(page, "requests")).toHaveText(["Mallory"]);
  await expect(mallory).toHaveCount(0);
  await expect(page.getByTestId("network-row")).toHaveCount(3);
  expect(errors).toEqual([]);
});

test("Add contact opens in the slot, in place of a dashpay: link's card; a row replaces either", async ({ page }) => {
  await openContacts(page);
  await page.getByTestId("contacts-add").click();
  const add = page.getByTestId("add-contact");
  await expect(add.getByRole("heading", { level: 2 })).toHaveText("Add a New Contact");
  await add.getByRole("button", { name: "Close" }).click();
  await expect(page.getByTestId("contact-pane-empty")).toBeVisible();

  await deliver(page, [{ kind: "addContact", identity: IDENTITY.bob, username: "bob" }]);
  await expect(page.getByTestId("add-contact-username")).toHaveText("@bob");
  // Add contact starts afresh: the link's candidate goes.
  await page.getByTestId("contacts-add").click();
  await expect(add).toBeVisible();
  await expect(page.getByTestId("add-contact-candidate")).toHaveCount(0);
  await row(page, "Bob Builder").click();
  await expect(page.getByTestId("contact-detail")).toBeVisible();
  await expect(add).toHaveCount(0);
});

test("IOS-072 no contacts: the empty state offers Search for a User; a pass before any snapshot says updating", async ({ page }) => {
  await openContacts(page, "dashpay-contacts-empty");
  await expect(page.getByTestId("contacts-empty")).toContainText("No contacts yet");
  await page.getByTestId("contacts-empty-search").click();
  await expect(page.getByTestId("add-contact")).toBeVisible();

  await openContacts(page, "dashpay-contacts-updating");
  await expect(page.getByTestId("contacts-updating")).toHaveText("Updating contacts…");
  await expect(page.getByTestId("contacts-empty")).toHaveCount(0);

  // After a snapshot, the rows stay, under the line.
  await openContacts(page, "dashpay-contacts-refreshing");
  await expect(page.getByTestId("contacts-updating-line")).toHaveText("Updating contacts…");
  await expect(page.getByTestId("contact-row")).toHaveCount(7);
});

test("a DashPay state that cannot be read says so and offers Try again", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", "light", "&fail=dashpay_status:rpc"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  const failed = page.getByTestId("contacts-status-error");
  await expect(failed).toContainText("Could not read DashPay's state");
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_status", null));
  await failed.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("contact-row")).toHaveCount(7);
  expect(errors).toEqual([]);
});

test("without an identity Contacts offers Join DashPay; while registering it says so", async ({ page }) => {
  await openContacts(page, "dashpay-no-identity");
  await expect(page.getByTestId("contacts-join")).toContainText("Join DashPay");
  await expect(page.getByTestId("contacts-add")).toHaveCount(0);
  await page.getByTestId("contacts-join-button").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();

  await openContacts(page, "dashpay-registering");
  await expect(page.getByTestId("contacts-registering")).toContainText("Your username is being registered");
});

// Every state of the page in light and dark, with 0 serious axe issues.
const STATES: Record<string, { scenario: string; go: (page: Page) => Promise<void> }> = {
  list: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await group(page, "hidden").getByRole("button").click();
      await row(page, "Erin").click();
      await page.getByTestId("contact-detail").waitFor();
    },
  },
  // Each chip tone on a selected row's tint.
  "selected-info": { scenario: "dashpay-registered", go: (page) => row(page, "Carol").click() },
  // DP2-09: a request's pane with its row actions, a contact's details and activity, the banner.
  "pane-request": { scenario: "dashpay-registered", go: (page) => row(page, "heidi").click() },
  "pane-contact": {
    scenario: "dashpay-registered",
    go: async (page) => {
      await row(page, "Bob Builder").click();
      await page.getByTestId("contact-activity-row").first().waitFor();
    },
  },
  "enable-banner": { scenario: "dashpay-no-keys", go: (page) => page.getByTestId("contacts-enable-fee").waitFor() },
  "selected-danger": { scenario: "dashpay-registered", go: (page) => row(page, "dave").click() },
  refreshing: { scenario: "dashpay-contacts-refreshing", go: (page) => page.getByTestId("contacts-updating-line").waitFor() },
  network: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await search(page).fill("ma");
      await page.getByTestId("contacts-network-more").click();
      await page.getByTestId("add-contact-hit").first().waitFor();
    },
  },
  "mixing-only": { scenario: "dashpay-contacts-mixing-only", go: (page) => page.getByTestId("contacts-setup-unlock").waitFor() },
  offline: {
    scenario: "dashpay-search-offline",
    go: async (page) => {
      await search(page).fill("ma");
      await page.getByTestId("contacts-network-error").waitFor();
    },
  },
  empty: { scenario: "dashpay-contacts-empty", go: (page) => page.getByTestId("contacts-empty").waitFor() },
  join: { scenario: "dashpay-no-identity", go: (page) => page.getByTestId("contacts-join").waitFor() },
};

for (const theme of THEMES) {
  for (const [name, { scenario, go }] of Object.entries(STATES)) {
    test(`contacts ${name} ${theme}: 0 serious axe issues`, async ({ page }, info) => {
      const errors = await openContacts(page, scenario, theme);
      await go(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-contacts-${name}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
