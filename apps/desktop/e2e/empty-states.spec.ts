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

  test(`notifications with nothing waiting offers a contact, and says there are no requests (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-bell-empty", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
    const requests = page.getByTestId("contact-requests-empty");
    await expect(requests).toContainText("No contact requests");
    await expect(requests.locator("[data-art=requests]")).toBeVisible();
    await expect(page.getByTestId("notifications-empty")).toContainText("No notifications yet");
    await expect(page.getByTestId("notifications-empty").locator("[data-art=notifications]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("contact-requests-add").click();
    await expect(page.getByTestId("add-contact")).toBeVisible();
  });

  test(`notifications without a username offer Join DashPay (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-no-identity", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
    const empty = page.getByTestId("notifications-no-identity");
    await expect(empty).toContainText("No notifications");
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

  test(`an empty sending address book offers Add address (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "addressBook");
    const empty = page.getByTestId("address-book-none");
    await expect(empty).toContainText("No sending addresses yet");
    await expect(empty.locator("[data-art=address]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("address-book-empty-add").click();
    await expect(page.getByTestId("address-book-editor")).toBeVisible();
  });

  test(`an empty receiving address book offers Receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "addressBook");
    await page.getByTestId("address-book-receive").click();
    const empty = page.getByTestId("address-book-none");
    await expect(empty).toContainText("No receiving addresses yet");
    await checked(page, errors);
    await page.getByTestId("address-book-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`receive with no payment requests focuses the form (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await page.getByTestId("sidebar-receive").click();
    const empty = page.getByTestId("requests-empty");
    await expect(empty).toContainText("No payment requests yet");
    await expect(empty.locator("[data-art=payment]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("requests-empty-create").click();
    await expect(page.getByTestId("request-label")).toBeFocused();
  });

  test(`CoinJoin before mixing offers Start Mixing (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
    await expect(page.getByTestId("coinjoin-status-title")).toHaveText("Not Started");
    await expect(page.locator("[data-testid=coinjoin-status] [data-art=coinjoin]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("coinjoin-action").click();
    await expect(page.locator("[data-testid=coinjoin-status][data-state=starting]")).toBeVisible();
  });

  test(`a wallet with no coins offers Receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await tools(page, "coinSelection");
    const empty = page.getByTestId("coins-none");
    await expect(empty).toContainText("This wallet has no coins yet");
    await expect(empty.locator("[data-art=coins]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("coins-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`pay with no contacts offers Add a contact (${theme})`, async ({ page }) => {
    const errors = await open(page, "dashpay-contacts-empty", theme);
    await page.getByTestId("sidebar-pay").click();
    const empty = page.getByTestId("pay-picker-empty");
    await expect(empty).toContainText("No contacts yet");
    await expect(empty.locator("[data-art=contacts]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("pay-picker-add").click();
    await expect(page.getByTestId("add-contact")).toBeVisible();
  });

  test(`no peers offers Change peers (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.evaluate(() => {
      window.__dwFixture!.world.tools.peers = [];
    });
    await tools(page, "peers");
    const empty = page.getByTestId("peers-none");
    await expect(empty).toContainText("No peers are connected");
    await expect(empty.locator("[data-art=peers]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("peers-empty-rotate").click();
    await expect(page.getByTestId("peers-notice")).toBeVisible();
  });
}
