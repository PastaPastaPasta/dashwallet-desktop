import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP2-09: the bell's popover and the Notifications page on the fixture backend, whose journal
// holds alice's events (src/fixture/dashpay.ts `journal`). IOS-075 is the parity ID: a request
// accepted from the bell, the events' lines, read on opening.

async function open(page: Page, scenario = "dashpay-registered", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const bell = (page: Page) => page.getByTestId("toolbar-bell");
const popover = (page: Page) => page.getByTestId("bell-popover");
const request = (page: Page, name: string) => page.getByTestId("bell-request").filter({ hasText: name });

/** Opens the popover, once the main identity is known (before, the bell opens the page). */
async function openBell(page: Page, by: "click" | "keyboard" = "click") {
  await expect(bell(page)).toHaveAttribute("aria-haspopup", "dialog");
  if (by === "click") await bell(page).click();
  else {
    await bell(page).focus();
    await page.keyboard.press("Enter");
  }
  await expect(popover(page).getByRole("heading", { name: "Notifications" })).toBeFocused();
}

test("IOS-075 the bell lists the requests and the new events, marks them read, and Escape gives focus back", async ({ page }) => {
  const errors = await open(page);
  await expect(page.getByTestId("toolbar-bell-badge")).toBeVisible();
  await expect(bell(page)).toHaveAttribute("aria-expanded", "false");
  await openBell(page);
  await expect(bell(page)).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByTestId("bell-request")).toHaveCount(2);
  await expect(request(page, "heidi")).toBeVisible();
  const fresh = page.getByTestId("bell-new").getByTestId("bell-event");
  await expect(fresh.filter({ hasText: "Bob Builder sent you a payment" })).toHaveCount(1);
  await expect(fresh.filter({ hasText: "sent you a contact request" })).toHaveCount(2);
  // Opening marks them read: the badge goes, the rows stay under New for this opening (D11).
  await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
  await expect(fresh.filter({ hasText: "Bob Builder sent you a payment" })).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(popover(page)).toHaveCount(0);
  await expect(bell(page)).toBeFocused();
  // The next opening has nothing new; the requests still wait.
  await openBell(page);
  await expect(page.getByTestId("bell-new")).toHaveCount(0);
  await expect(page.getByTestId("bell-request")).toHaveCount(2);
  expect(errors).toEqual([]);
});

test("IOS-075 accepts a request from the bell under a passphrase grant", async ({ page }) => {
  const errors = await open(page);
  await openBell(page);
  await expect(request(page, "heidi").getByTestId("bell-accept")).toHaveAccessibleName("Accept heidi");
  await request(page, "heidi").getByTestId("bell-accept").click();
  await expect(page.getByTestId("grant-sheet")).toBeVisible();
  await authorize(page);
  await expect(page.getByTestId("bell-outcome")).toHaveText("You and heidi are now contacts.");
  await expect(request(page, "heidi")).toHaveCount(0);
  await expect(page.getByTestId("bell-request")).toHaveCount(1);
  // The popover stayed open through the passphrase sheet; Contacts lists heidi as a contact.
  await page.keyboard.press("Escape");
  await page.getByTestId("sidebar-contacts").click();
  await expect(page.getByTestId("contacts-group-contacts").locator(".contact-row-name")).toContainText(["heidi"]);
  expect(errors).toEqual([]);
});

test("D4 an accept that ends after the popover closed says so in a toast", async ({ page }) => {
  await open(page);
  await openBell(page);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("dashpay_accept_request"));
  await request(page, "heidi").getByTestId("bell-accept").click();
  await authorize(page);
  // The fixture applies the accept and holds its reply: the action still runs.
  await expect(request(page, "Grace Hopper").getByTestId("bell-accept")).toBeDisabled();
  await bell(page).click();
  await expect(popover(page)).toHaveCount(0);
  await release.evaluate((r) => r());
  await expect(page.locator(".toast")).toHaveText("You and heidi are now contacts.");
});

test("D27 a wallet switch from the keyboard closes the bell; the other wallet's events stay unread", async ({ page }) => {
  const errors = await open(page, "dashpay-two-wallets");
  const badge = page.getByTestId("toolbar-bell-badge");
  const picker = page.getByTestId("toolbar-wallet-picker");
  await expect(badge).toBeVisible();
  await openBell(page, "keyboard");
  await expect(badge).toHaveCount(0);
  // Back to the picker with the keyboard (no click outside the popover), and the other wallet.
  await page.keyboard.press("Shift+Tab");
  await expect(bell(page)).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(picker).toBeFocused();
  await picker.selectOption({ index: 1 });
  // Its bell, once its identity is known, stays closed and its events unread.
  await expect(bell(page)).toHaveAttribute("aria-haspopup", "dialog");
  await expect(bell(page)).toHaveAttribute("aria-expanded", "false");
  await expect(badge).toBeVisible();
  await expect(popover(page)).toHaveCount(0);
  await page.getByTestId("sidebar-home").click();
  await expect(badge).toBeVisible();
  // Wallet A's bell is read; reopening wallet A does not reopen it.
  await picker.selectOption({ index: 0 });
  await expect(bell(page)).toHaveAttribute("aria-haspopup", "dialog");
  await expect(popover(page)).toHaveCount(0);
  await expect(badge).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("D27 an accept that ends after a wallet switch says so under its wallet's name", async ({ page }) => {
  await open(page, "dashpay-two-wallets");
  const picker = page.getByTestId("toolbar-wallet-picker");
  const walletA = (await picker.locator("option").first().textContent())!;
  await openBell(page);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("dashpay_accept_request"));
  await request(page, "heidi").getByTestId("bell-accept").click();
  await authorize(page);
  await picker.selectOption({ index: 1 });
  await expect(popover(page)).toHaveCount(0);
  await release.evaluate((r) => r());
  await expect(page.locator(".toast")).toHaveText(`${walletA}: You and heidi are now contacts.`);
  // The accept was wallet A's: wallet B's request from heidi still waits.
  await openBell(page);
  await expect(request(page, "heidi")).toHaveCount(1);
});

test("D28 after a keyboard accept, focus goes to the next request, then to the popover's heading", async ({ page }) => {
  const errors = await open(page);
  await openBell(page, "keyboard");
  await request(page, "heidi").getByTestId("bell-accept").focus();
  await page.keyboard.press("Enter");
  await authorize(page);
  await expect(page.getByTestId("bell-outcome")).toHaveText("You and heidi are now contacts.");
  await expect(request(page, "heidi")).toHaveCount(0);
  await expect(request(page, "Grace Hopper").locator(".bell-row-open")).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(request(page, "Grace Hopper").getByTestId("bell-accept")).toBeFocused();
  await page.keyboard.press("Enter");
  await authorize(page);
  await expect(page.getByTestId("bell-request")).toHaveCount(0);
  await expect(popover(page).getByRole("heading", { name: "Notifications" })).toBeFocused();
  expect(errors).toEqual([]);
});

test("D28 on the page, an ignored request hands focus to the next one, the last to the heading", async ({ page }) => {
  await open(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
  const requests = page.getByTestId("notifications-requests");
  await requests.getByTestId("bell-request").first().waitFor();
  await requests.getByTestId("bell-ignore").first().focus();
  await page.keyboard.press("Enter");
  await expect(requests.getByTestId("bell-request")).toHaveCount(1);
  await expect(requests.locator(".bell-row-open")).toBeFocused();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Enter");
  await authorize(page);
  await expect(requests).toHaveCount(0);
  await expect(page.getByTestId("page-notifications").locator("h1")).toBeFocused();
});

test("D12 an event opens what it is about, and a click outside closes the popover", async ({ page }) => {
  await open(page);
  await openBell(page);
  await page.getByTestId("bell-event").filter({ hasText: "Bob Builder sent you a payment" }).getByRole("button").click();
  await expect(popover(page)).toHaveCount(0);
  await expect(page.getByTestId("tx-detail")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("tx-detail")).toHaveCount(0);

  await openBell(page);
  await request(page, "Grace Hopper").getByRole("button", { name: /Grace Hopper/ }).first().click();
  await expect(page.getByTestId("contact-detail").getByRole("heading", { level: 2 })).toHaveText("Grace Hopper");

  await openBell(page);
  await page.getByTestId("page-contacts").locator("h1").click();
  await expect(popover(page)).toHaveCount(0);
});

test("D12 the page lists the requests, New and Earlier, and marks New read", async ({ page }) => {
  const errors = await open(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
  await expect(page.getByTestId("page-notifications")).toBeVisible();
  await expect(page.getByTestId("notifications-requests").getByTestId("bell-request")).toHaveCount(2);
  await expect(page.getByTestId("notifications-new").getByTestId("bell-event").filter({ hasText: "Bob Builder sent you a payment" })).toHaveCount(1);
  await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
  const earlier = page.getByTestId("notifications-earlier");
  await expect(earlier.getByTestId("bell-event").filter({ hasText: "Your username @alice is registered" })).toHaveCount(1);
  await expect(earlier.getByTestId("bell-event").filter({ hasText: "You lost the vote for @al" })).toHaveCount(1);
  expect(errors).toEqual([]);
});

test("an empty bell says so, and so does the page", async ({ page }) => {
  await open(page, "dashpay-bell-empty");
  await openBell(page);
  await expect(page.getByTestId("bell-empty")).toHaveText("No new notifications");
  await page.getByTestId("bell-see-all").click();
  await expect(page.getByTestId("notifications-empty")).toBeVisible();
});

test("without an identity the bell opens the page, which says why it is empty", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await bell(page).click();
  await expect(page.getByTestId("notifications-no-identity")).toBeVisible();
});

const STATES: Record<string, { scenario: string; go: (page: Page) => Promise<unknown> }> = {
  popover: { scenario: "dashpay-registered", go: (page) => openBell(page) },
  page: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
      await page.getByTestId("notifications-new").waitFor();
    },
  },
};

for (const theme of THEMES) {
  for (const [name, { scenario, go }] of Object.entries(STATES)) {
    test(`notifications ${name} ${theme}: 0 serious axe issues`, async ({ page }, info) => {
      const errors = await open(page, scenario, theme);
      await go(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-notifications-${name}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
