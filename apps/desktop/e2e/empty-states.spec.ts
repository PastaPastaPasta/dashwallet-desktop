// Empty screens: each has its own drawing and one next step (PRODUCT.md #11).

import { expect, test, type Page } from "@playwright/test";
import type { ToolsTab } from "../src/api/generated/ToolsTab";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

const WALLET = "a1".repeat(32);

async function open(page: Page, scenario: string, theme: Theme, extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function tools(page: Page, tab: ToolsTab) {
  await page.evaluate((t) => {
    (document.activeElement as HTMLElement | null)?.blur();
    window.__dwFixture!.shellCommand({ kind: "tools", tab: t });
  }, tab);
  await expect(page.getByTestId(`tools-tabs-${tab}`)).toHaveAttribute("aria-selected", "true");
}

async function checked(page: Page, errors: string[]) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
}

for (const theme of THEMES) {
  test(`home history empty offers Receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    const empty = page.getByTestId("history-empty");
    await expect(empty).toContainText("There are no transactions to display");
    await expect(empty.locator("[data-art=activity]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("history-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`activity empty offers Receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme, "&fail=platform_activity:none&fail=shielded_history:none");
    await page.getByTestId("sidebar-activity").click();
    const empty = page.getByTestId("tx-empty");
    await expect(empty).toContainText("There are no transactions to display");
    await expect(empty.locator("[data-art=activity]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("tx-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`activity filters that match nothing can be cleared (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await page.getByTestId("tx-category-sent").click();
    const empty = page.getByTestId("tx-empty");
    await expect(empty).toContainText("Nothing matches these filters");
    await expect(empty.locator("[data-art=search]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("tx-empty-clear").click();
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await expect(page.getByTestId("tx-empty")).toHaveCount(0);
  });

  test(`contacts empty offers Add contact (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-contacts-empty", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    const empty = page.getByTestId("contacts-empty");
    await expect(empty).toContainText("No contacts yet");
    await expect(empty.locator("[data-art=contacts]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("contacts-empty-search").click();
    await expect(page.getByTestId("add-contact")).toBeVisible();
  });

  test(`notifications with nothing waiting is one card that offers Add contact (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-bell-empty", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
    const empty = page.getByTestId("notifications-empty");
    await expect(empty).toContainText("No notifications yet");
    await expect(empty).toContainText("Contact requests and payments from your contacts show up here.");
    await expect(empty.locator("[data-art=notifications]")).toBeVisible();
    await expect(page.getByTestId("contact-requests-empty")).toHaveCount(0);
    await expect(page.locator("[data-testid=page-notifications] .empty-state")).toHaveCount(1);
    await checked(page, errors);
    await page.getByTestId("notifications-add-contact").click();
    await expect(page.getByTestId("add-contact")).toBeVisible();
  });

  test(`notifications without a username offer Join DashPay (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-no-identity", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
    const empty = page.getByTestId("notifications-no-identity");
    await expect(empty).toContainText("No notifications yet");
    await expect(empty.locator("[data-art=notifications]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("notifications-join").click();
    await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  });

  test(`the bell's empty note keeps See all (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-bell-empty", theme);
    await page.getByTestId("toolbar-bell").click();
    const empty = page.getByTestId("bell-empty");
    await expect(empty).toContainText("No new notifications");
    await expect(empty.locator("[data-art=notifications]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("bell-see-all").click();
    await expect(page.getByTestId("page-notifications")).toBeVisible();
  });

  test(`an empty sending address book offers Add address and hides New (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "addressBook");
    const empty = page.getByTestId("address-book-none");
    await expect(empty).toContainText("No sending addresses yet");
    await expect(empty.locator("[data-art=address]")).toBeVisible();
    await expect(page.getByTestId("address-book-new")).toHaveCount(0);
    await checked(page, errors);
    await page.getByTestId("address-book-empty-add").click();
    await expect(page.getByTestId("address-book-editor")).toBeVisible();
  });

  test(`an empty receiving address book offers Receive Dash (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "addressBook");
    await page.getByTestId("address-book-receive").click();
    const empty = page.getByTestId("address-book-none");
    await expect(empty).toContainText("No receiving addresses yet");
    await expect(page.getByTestId("address-book-empty-receive")).toHaveText("Receive Dash");
    await checked(page, errors);
    await page.getByTestId("address-book-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`receive with no payment requests is one line (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await page.getByTestId("sidebar-receive").click();
    const empty = page.getByTestId("requests-empty");
    await expect(empty).toHaveText("Requests you create show up here.");
    await expect(empty.locator("[data-art]")).toHaveCount(0);
    await checked(page, errors);
  });

  test(`a wallet with no coins offers Receive Dash (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await tools(page, "coinSelection");
    const empty = page.getByTestId("coins-none");
    await expect(empty).toContainText("No coins yet");
    await expect(empty).toContainText("Coins you receive show up here.");
    await expect(empty.locator("[data-art=coins]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("coins-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`pay with no contacts keeps the username search and shows no add action (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-contacts-empty", theme);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-contact").click();
    const empty = page.getByTestId("pay-picker-empty");
    await expect(empty).toContainText("No contacts yet");
    await expect(empty).toContainText("Type a username above to find someone on DashPay.");
    await expect(empty.locator("[data-art=contacts]")).toBeVisible();
    await expect(page.getByTestId("pay-picker-add")).toHaveCount(0);
    await checked(page, errors);
  });

  test(`no peers explains itself and does not nest a card (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate(() => {
      window.__dwFixture!.world.tools.peers = [];
    });
    await tools(page, "peers");
    const empty = page.getByTestId("peers-none");
    await expect(empty).toContainText("No peers are connected");
    await expect(empty).toContainText("Check your internet connection. The wallet keeps looking for peers.");
    await expect(empty.locator("[data-art=peers]")).toBeVisible();
    await expect(empty).not.toHaveClass(/card-l1/);
    await expect(page.getByTestId("peers-empty-rotate")).toHaveCount(0);
    await expect(page.getByTestId("peers-rotate")).toBeVisible();
    await checked(page, errors);
  });

  test(`history stays quiet while the wallet is syncing (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-syncing", theme, "&fail=platform_activity:none&fail=shielded_history:none");
    const home = page.getByTestId("history-syncing");
    await expect(home).toContainText("Your history is still syncing");
    await expect(home).toContainText("Payments show up here as the wallet catches up.");
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    const activity = page.getByTestId("tx-syncing");
    await expect(activity).toContainText("Your history is still syncing");
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await expect(page.getByTestId("tx-export-why")).toHaveCount(0);
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    await checked(page, errors);
  });

  test(`clearing Activity's filters focuses the search (${theme})`, async ({ page }) => {
    await open(page, "funded", theme);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-search").locator("input").fill("zzzz-no-such-tx");
    await expect(page.getByTestId("tx-empty")).toContainText("Nothing matches these filters");
    await page.getByTestId("tx-empty-clear").click();
    await expect(page.getByTestId("tx-search").locator("input")).toBeFocused();
    await expect(page.getByTestId("tx-list")).toBeVisible();
  });

  test(`clearing a contact search focuses the search (${theme})`, async ({ page }) => {
    await open(page, "dashpay-registered", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    await page.getByTestId("contacts-search").locator("input").fill("zzz");
    await expect(page.getByTestId("contacts-no-match")).toBeVisible();
    await page.getByTestId("contacts-no-match-clear").click();
    await expect(page.getByTestId("contacts-search").locator("input")).toBeFocused();
    await expect(page.getByTestId("contact-row").first()).toBeVisible();
  });

  test(`contacts with nobody hide the header button and the detail pane (${theme})`, async ({ page }) => {
    await open(page, "dashpay-contacts-empty", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    await expect(page.getByTestId("contacts-empty")).toBeVisible();
    await expect(page.getByTestId("contacts-add")).toHaveCount(0);
    await expect(page.getByTestId("contact-pane-empty")).toHaveCount(0);
    await expect(page.getByTestId("contacts-split")).toHaveClass(/is-solo/);
  });

  test(`clearing the pay picker search focuses the search (${theme})`, async ({ page }) => {
    await open(page, "dashpay-registered", theme);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-contact").click();
    await page.getByTestId("pay-contact-search").locator("input").fill("z");
    await expect(page.getByTestId("pay-picker-none")).toContainText("No contacts match");
    await page.getByTestId("pay-picker-clear").click();
    await expect(page.getByTestId("pay-contact-search").locator("input")).toBeFocused();
    await expect(page.getByTestId("pay-picker-row").first()).toBeVisible();
  });

  test(`clearing an address-book search focuses the search (${theme})`, async ({ page }) => {
    await open(page, "funded", theme);
    await tools(page, "addressBook");
    await page.getByTestId("address-book-search").fill("zzzz-no-such");
    await expect(page.getByTestId("address-book-none")).toContainText("No entry matches the search");
    await page.getByTestId("address-book-empty-clear").click();
    await expect(page.getByTestId("address-book-search")).toBeFocused();
    await expect(page.getByTestId("address-book-table")).toBeVisible();
  });

  test(`signing's address picker does not offer Receive (${theme})`, async ({ page }) => {
    await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "signVerify");
    await page.getByTestId("sign-choose").click();
    const picker = page.getByTestId("address-book-picker");
    await expect(picker).toContainText("Choose the address to sign with");
    await expect(picker.getByTestId("address-book-none")).toContainText("No receiving addresses yet");
    await expect(picker.getByTestId("address-book-empty-receive")).toHaveCount(0);
    await expect(picker.getByRole("button", { name: "Receive Dash" })).toHaveCount(0);
  });

  test(`showing every mixed coin focuses the first filter (${theme})`, async ({ page }) => {
    await open(page, "coins-all-mixed", theme);
    await tools(page, "coinSelection");
    const empty = page.getByTestId("coins-none");
    await expect(empty).toContainText("All your coins are mixed");
    await expect(empty).toContainText("Mixed coins are hidden from this list.");
    await page.getByTestId("coins-empty-show").click();
    await expect(page.getByTestId("coins-mode-list")).toBeFocused();
    await expect(page.getByTestId("coin-table")).toBeVisible();
  });

  test(`a failed wallet list does not offer to create a wallet (${theme})`, async ({ page }) => {
    await open(page, "funded", theme, "&fail=list_wallets:none");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "addressBook" }));
    await expect(page.getByTestId("page-wallets-error")).toBeVisible();
    await expect(page.getByTestId("tools-addressBook-no-wallet")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Create a wallet" })).toHaveCount(0);
  });

  test(`an empty wallet list offers to create a wallet on a tool (${theme})`, async ({ page }) => {
    await open(page, "no-wallet", theme);
    await expect(page.getByTestId("no-wallet")).toContainText("Create or restore a wallet to get started.");
    await expect(page.getByTestId("no-wallet").getByRole("button")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "addressBook" }));
    const empty = page.getByTestId("tools-addressBook-no-wallet");
    await expect(empty).toContainText("Create or restore a wallet to use this tool.");
    await expect(empty.getByRole("button", { name: "Create a wallet" })).toHaveCount(1);
  });
}
