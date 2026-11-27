// Empty screens: each has its own drawing and one next step (PRODUCT.md #11).

import { expect, test, type Page } from "@playwright/test";
import { CREATED_12 } from "../src/fixture/vault";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, openToolsTab as tools, THEMES, trackPageErrors, type Theme } from "./helpers";

const WALLET = "a1".repeat(32);

async function open(page: Page, scenario: string, theme: Theme, extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function checked(page: Page, errors: string[]) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
}

/** Picks the asked words of `phrase` in order on the verify step. */
async function verifyPhrase(page: Page, phrase: string) {
  const words = phrase.split(" ");
  const labels = await page.getByTestId("verify-slot").locator(".verify-slot-label").allTextContents();
  for (const label of labels) {
    const position = Number(label.replace("Word ", "")) - 1;
    await page.getByTestId("verify-chip").filter({ hasText: new RegExp(`^${words[position]}$`) }).click();
  }
}

/** Home, Activity (plain and filtered) and Coin Selection must not treat an empty list as final. */
async function expectUnsettled(page: Page, historyTitle: string, coinsTitle: string) {
  await expect(page.getByTestId("history-syncing")).toContainText(historyTitle);
  await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
  await page.getByTestId("sidebar-activity").click();
  await expect(page.getByTestId("tx-syncing")).toContainText(historyTitle);
  await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
  await page.getByTestId("tx-category-sent").click();
  await expect(page.getByTestId("tx-syncing")).toContainText(historyTitle);
  await expect(page.getByText("Nothing matches these filters")).toHaveCount(0);
  await expect(page.getByTestId("tx-empty-clear")).toHaveCount(0);
  await expect(page.getByTestId("tx-export-why")).toHaveCount(0);
  await expect(page.getByTestId("tx-footer")).toHaveCount(0);
  await tools(page, "coinSelection");
  await expect(page.getByTestId("coins-none")).toContainText(coinsTitle);
  await expect(page.getByTestId("coins-empty-receive")).toHaveCount(0);
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
    const errors = await open(page, "empty-activity", theme);
    await page.getByTestId("sidebar-activity").click();
    const empty = page.getByTestId("tx-empty");
    await expect(empty).toContainText("There are no transactions to display");
    await expect(empty.locator("[data-art=activity]")).toBeVisible();
    await checked(page, errors);
    await page.getByTestId("tx-empty-receive").click();
    await expect(page.getByTestId("page-receive")).toBeVisible();
  });

  test(`activity is not empty until Platform history has answered (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme, "&hold=platform_activity");
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.release("platform_activity"));
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`a filtered activity waits for Platform history before it says nothing matches (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme, "&hold=platform_activity");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("tx-empty-clear")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.release("platform_activity"));
    await expect(page.getByTestId("tx-empty")).toContainText("Nothing matches these filters");
    expect(errors).toEqual([]);
  });

  test(`a filtered activity waits for Core history while the wallet is syncing (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-syncing", theme);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-category-sent").click();
    await expect(page.getByTestId("tx-syncing")).toContainText("Your history is still syncing");
    await page.getByTestId("tx-category-all").click();
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("tx-empty")).toHaveCount(0);
    await expect(page.getByTestId("tx-empty-clear")).toHaveCount(0);
    await expect(page.getByTestId("tx-export-why")).toHaveCount(0);
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    expect(errors).toEqual([]);
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

  test(`an address book that could not be read, or a search of an empty one, keeps New (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme, "&fail=address_book:none");
    await tools(page, "addressBook");
    await expect(page.getByTestId("address-book-problem")).toBeVisible();
    await expect(page.getByTestId("address-book-empty-add")).toHaveCount(0);
    await page.getByTestId("address-book-new").click();
    await expect(page.getByTestId("address-book-editor")).toBeVisible();
    await page.goto("about:blank");
    await open(page, "funded", theme);
    await page.evaluate((id) => {
      window.__dwFixture!.world.tools.addressBook[id] = [];
    }, WALLET);
    await tools(page, "addressBook");
    await expect(page.getByTestId("address-book-empty-add")).toBeVisible();
    await page.getByTestId("address-book-search").fill("rent");
    await expect(page.getByTestId("address-book-empty-clear")).toBeVisible();
    await expect(page.getByTestId("address-book-new")).toBeVisible();
    expect(errors).toEqual([]);
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
    await page.getByTestId("pay-contacts").click();
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
    const errors = await open(page, "empty-syncing", theme);
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

  test(`dismissing Pay suggestions leaves the recipient field focused (${theme})`, async ({ page }) => {
    await open(page, "dashpay-registered", theme);
    await page.getByTestId("sidebar-pay").click();
    const field = page.getByTestId("pay-address");
    await field.fill("bob");
    await expect(page.getByTestId("pay-picker-row").first()).toBeVisible();
    await field.press("Escape");
    await expect(field).toBeFocused();
    await expect(page.getByTestId("pay-recipient-popover")).toHaveCount(0);
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

  test(`showing hidden CoinJoin coins focuses the first filter (${theme})`, async ({ page }) => {
    await open(page, "coins-all-mixed", theme);
    await tools(page, "coinSelection");
    const empty = page.getByTestId("coins-none");
    await expect(empty).toContainText("Every coin here is a CoinJoin coin");
    await expect(empty).toContainText("CoinJoin coins are hidden from this list.");
    await page.getByTestId("coins-empty-show").click();
    await expect(page.getByTestId("coins-mode-list")).toBeFocused();
    await expect(page.getByTestId("coin-table")).toBeVisible();
  });

  test(`a failed wallet list does not offer to create a wallet (${theme})`, async ({ page }) => {
    await open(page, "funded", theme, "&fail=list_wallets:none");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "addressBook" }));
    await expect(page.getByTestId("wallets-error")).toBeVisible();
    await expect(page.getByTestId("tools-addressBook-no-wallet")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Create a wallet" })).toHaveCount(0);
    // The tools that need no wallet stay open.
    await tools(page, "information");
    await expect(page.getByTestId("tools-information")).toBeVisible();
    await tools(page, "psbt");
    await expect(page.getByTestId("wallets-error")).toHaveCount(0);
  });

  test(`a failed refetch of the wallet list keeps the page and what was typed (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.getByTestId("sidebar-pay").click();
    const amount = page.getByTestId("pay-amount");
    await amount.fill("0.5");
    const before = await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "list_wallets").length);
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.fail("list_wallets", "none");
      const w = f.world.wallets[0].summary;
      f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.walletId, balances: w.balances });
    });
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "list_wallets").length)).toBeGreaterThan(before);
    await expect(page.getByTestId("page-wallets-error")).toHaveCount(0);
    await expect(amount).toHaveValue("0.5");
    expect(errors).toEqual([]);
  });

  test(`a saved first wallet whose list cannot be read stays on the ready step and retries (${theme})`, async ({ page }) => {
    const errors = await open(page, "no-wallet-mainnet", theme);
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    // The first list read succeeded and was empty. The refresh after the save fails.
    await page.evaluate(() => window.__dwFixture!.fail("list_wallets", "none"));
    await page.getByTestId("onboarding-create").click();
    await page.locator(".onboarding-advanced summary").click();
    await page.getByTestId("onboarding-skip-passphrase").click();
    await page.getByTestId("onboarding-written-down").check();
    await page.getByTestId("onboarding-phrase-continue").click();
    await verifyPhrase(page, CREATED_12);
    await expect(page.getByTestId("onboarding-done")).toBeVisible();
    await expect(page.getByTestId("onboarding-welcome")).toHaveCount(0);
    await expect(page.getByTestId("onboarding-rates")).toBeVisible();
    await expect(page.getByTestId("onboarding-list-error")).toContainText("could not be read");
    const retry = page.getByTestId("onboarding-list-retry");
    await expect(retry).toBeEnabled();
    // The price disclosure's Open path must not leave the button disabled when the read fails again.
    await retry.click();
    await expect(page.getByTestId("onboarding-done")).toBeVisible();
    await expect(retry).toBeEnabled();
    await page.evaluate(() => window.__dwFixture!.fail("list_wallets", null));
    await retry.click();
    await expect(page.getByTestId("onboarding")).toHaveCount(0);
    await expect(page.getByTestId("hero")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`home, activity and coin selection wait while Core has not caught up (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme);
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const s = f.world.sync;
      f.world.sync = { ...s, running: false, caughtUp: false, activePhase: null, connectedPeers: 0 };
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expectUnsettled(page, "Syncing has stopped", "Syncing has stopped");
    expect(errors).toEqual([]);
  });

  test(`a failed history read is not an empty wallet (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme, "&fail=history_page:none");
    const failed = page.getByTestId("history-error");
    await expect(failed).toContainText("Could not load transactions");
    await expect(failed.getByRole("button", { name: "Try again" })).toBeVisible();
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.fail("history_page", null));
    await failed.getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("overview-group-0")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`an empty list follows the engine, not a display that still says caught up (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const s = f.world.sync;
      // One block behind a caught-up wallet: the status bar holds "up to date", the engine does not.
      f.world.sync = {
        ...s,
        running: true,
        caughtUp: false,
        activePhase: "filters",
        phases: s.phases.map((p) => ({ ...p, currentHeight: (p.targetHeight ?? 1) - 1, done: false })),
      };
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("history-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`an event that says behind is followed though the read after it fails (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const s = f.world.sync;
      f.fail("sync_status", "none");
      f.world.sync = {
        ...s,
        running: true,
        caughtUp: false,
        activePhase: "filters",
        phases: s.phases.map((p) => ({ ...p, currentHeight: (p.targetHeight ?? 1) - 1, done: false })),
      };
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("history-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await tools(page, "coinSelection");
    await expect(page.getByTestId("coins-none")).toContainText("Coins are still syncing");
    await expect(page.getByTestId("coins-empty-receive")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`an empty history whose next read fails asks to retry, not to receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.evaluate((walletId) => {
      const f = window.__dwFixture!;
      f.fail("history_page", "none");
      f.emit({ kind: "historyChanged", network: f.world.launch.network.dirName, walletId, txids: [] });
    }, WALLET);
    const failed = page.getByTestId("history-error");
    await expect(failed).toContainText("Could not load transactions");
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-retry")).toBeVisible();
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.fail("history_page", null));
    await page.getByTestId("tx-retry").click();
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`a wallet whose scan is behind the tip is not an empty wallet (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      (f.world.wallets[0] as { scanHeight?: number | null }).scanHeight = 1;
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("history-syncing")).toContainText("Your history is still syncing");
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`a sync event during the first scan-height read is read again (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.evaluate((walletId) => {
      const f = window.__dwFixture!;
      (f.world.wallets[0] as { scanHeight?: number | null }).scanHeight = 1;
      (window as unknown as { __release: () => void }).__release = f.hold("wallet_scan_height");
      f.emit({ kind: "walletLoadChanged", network: f.world.launch.network.dirName, walletId, loaded: true });
    }, WALLET);
    await expect(page.getByTestId("history-syncing")).toContainText("The wallet is still starting");
    const before = await page.evaluate(() => {
      const f = window.__dwFixture!;
      delete (f.world.wallets[0] as { scanHeight?: number | null }).scanHeight;
      const n = f.calls.length;
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
      return n;
    });
    // The event's own reads have started, so its invalidation met the held read.
    await expect.poll(() => page.evaluate((n) => window.__dwFixture!.calls.slice(n).includes("sync_status"), before)).toBe(true);
    await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`activity keeps rows found while catching up, but does not count them (${theme})`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-footer")).toContainText("transactions");
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      (f.world.wallets[0] as { scanHeight?: number | null }).scanHeight = 1;
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      delete (f.world.wallets[0] as { scanHeight?: number | null }).scanHeight;
      f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
    });
    await expect(page.getByTestId("tx-footer")).toContainText("transactions");
    expect(errors).toEqual([]);
  });

  test(`an empty list being read again after a payment does not offer Receive (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme);
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    const changed = (command: "history_page" | "utxos") =>
      page.evaluate(
        ({ walletId, command }) => {
          const f = window.__dwFixture!;
          (window as unknown as { __release: () => void }).__release = f.hold(command);
          f.emit({ kind: "historyChanged", network: f.world.launch.network.dirName, walletId, txids: [] });
        },
        { walletId: WALLET, command },
      );
    const release = () => page.evaluate(() => (window as unknown as { __release: () => void }).__release());
    await changed("history_page");
    await expect(page.getByTestId("history-loading")).toBeVisible();
    await expect(page.getByTestId("history-empty-receive")).toHaveCount(0);
    await release();
    await expect(page.getByTestId("history-empty-receive")).toBeVisible();
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    await changed("history_page");
    await expect(page.getByTestId("tx-loading")).toBeVisible();
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await release();
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    await tools(page, "coinSelection");
    await expect(page.getByTestId("coins-empty-receive")).toBeVisible();
    await changed("utxos");
    await expect(page.getByTestId("coins-loading")).toBeVisible();
    await expect(page.getByTestId("coins-empty-receive")).toHaveCount(0);
    await release();
    await expect(page.getByTestId("coins-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  for (const [where, prefix, receive, card] of [
    ["home", "history", "history-empty-receive", "history-syncing"],
    ["activity", "tx", "tx-empty-receive", "tx-syncing"],
    ["coin selection", "coins", "coins-empty-receive", "coins-none"],
  ] as const) {
    test(`a catch-up that could not be read offers Try again on ${where}, which reads it again (${theme})`, async ({ page }) => {
      const errors = await open(page, "empty-activity", theme, "&fail=wallet_scan_height:none");
      if (where === "activity") await page.getByTestId("sidebar-activity").click();
      if (where === "coin selection") await tools(page, "coinSelection");
      await expect(page.getByTestId(card)).toContainText("Could not tell whether the wallet has caught up");
      await expect(page.getByTestId(receive)).toHaveCount(0);
      await page.evaluate(() => window.__dwFixture!.fail("wallet_scan_height", null));
      await page.getByTestId(`${prefix}-recheck`).click();
      await expect(page.getByTestId(receive)).toBeVisible();
      expect(errors).toEqual([]);
    });
  }

  test(`home, activity and coin selection wait while Core is scanning (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-syncing", theme);
    await expectUnsettled(page, "Your history is still syncing", "Coins are still syncing");
    expect(errors).toEqual([]);
  });

  test(`a rejected Platform read is not an empty activity (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme, "&fail=platform_activity:none");
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-error")).toContainText("Could not load transactions");
    await expect(page.getByTestId("tx-retry")).toBeVisible();
    await expect(page.getByTestId("tx-empty")).toHaveCount(0);
    await expect(page.getByTestId("tx-empty-receive")).toHaveCount(0);
    await expect(page.getByTestId("tx-export-why")).toHaveCount(0);
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    await expect(page.getByText("Nothing matches these filters")).toHaveCount(0);
    await checked(page, errors);
    await page.getByTestId("tx-date").selectOption("today");
    await expect(page.getByTestId("tx-error")).toBeVisible();
    await expect(page.getByText("Nothing matches these filters")).toHaveCount(0);
    await expect(page.getByTestId("tx-export-why")).toHaveCount(0);
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.fail("platform_activity", null));
    await page.getByTestId("tx-retry").click();
    await expect(page.getByTestId("tx-error")).toHaveCount(0);
    await expect(page.getByTestId("tx-empty")).toContainText("Nothing matches these filters");
    await expect(page.getByTestId("tx-footer")).toHaveText("0 transactions");
    await page.getByTestId("tx-date").selectOption("all");
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`absent commands and unsupported optional pools still settle an empty activity (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty-activity", theme, "&fail=platform_activity:call&fail=shielded_history:protocol");
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-error")).toHaveCount(0);
    await expect(page.getByTestId("tx-empty-receive")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`a failed Core read stays visible when Platform has a row (${theme})`, async ({ page }) => {
    const errors = await open(page, "empty", theme, "&fail=history_page:none");
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-error")).toContainText("Could not load transactions");
    await expect(page.getByTestId("tx-retry")).toBeVisible();
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await expect(page.getByTestId("tx-empty")).toHaveCount(0);
    await expect(page.getByTestId("tx-footer")).toHaveCount(0);
    await checked(page, errors);
    await page.evaluate(() => window.__dwFixture!.fail("history_page", null));
    await page.getByTestId("tx-retry").click();
    await expect(page.getByTestId("tx-error")).toHaveCount(0);
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await expect(page.getByTestId("tx-footer")).toBeVisible();
    expect(errors).toEqual([]);
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

test("activity shows no rows before Core history answers, so the first row opens its detail", async ({ page }) => {
  const errors = await open(page, "funded", "light");
  await page.getByTestId("hero").waitFor();
  await page.evaluate(() => void ((window as unknown as { __release: () => void }).__release = window.__dwFixture!.hold("history_page")));
  const asked = await page.evaluate(() => window.__dwFixture!.calls.length);
  await page.getByTestId("sidebar-activity").click();
  await expect(page.getByTestId("tx-loading")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate((n) => {
        const calls = window.__dwFixture!.calls;
        return calls.slice(n).includes("history_page") && ["platform_activity", "shielded_history"].every((c) => calls.includes(c));
      }, asked),
    )
    .toBe(true);
  // Platform and Shielded have answered and rendered; Core is still held.
  await page.evaluate(() => new Promise((r) => setTimeout(() => requestAnimationFrame(() => requestAnimationFrame(r)), 0)));
  expect(await page.locator("[data-txid]").count()).toBe(0);
  await expect(page.getByTestId("tx-loading")).toBeVisible();
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await page.getByTestId("tx-list").waitFor();
  await page.locator("[data-txid]").first().click();
  await expect(page.getByTestId("tx-detail-facts")).toBeVisible();
  expect(errors).toEqual([]);
});

test("removing a wallet and restoring the same phrase forgets its cached scan height without a sync event", async ({ page }) => {
  const errors = await open(page, "funded", "light");
  await page.evaluate(() => {
    const w = window.__dwFixture!.world;
    if (w.pending.platform) w.pending.platform.activity = [];
    if (w.pending.shielded) w.pending.shielded.activity = [];
    window.__dwFixture!.shellCommand({ kind: "addWallet" });
  });
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(CREATED_12);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await page.getByTestId("onboarding-open").click();
  const firstId = await page.evaluate(() => window.__dwFixture!.world.wallets.at(-1)!.summary.walletId);
  await expect(page.getByTestId("history-empty-receive")).toBeVisible();

  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-wallets").click();
  await page.getByTestId(`settings-wallet-${firstId.slice(0, 8)}-remove`).click();
  await page.getByTestId("settings-remove-passphrase-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("settings-remove-passphrase-submit").click();
  await expect(page.getByTestId("settings-wallets-notice")).toBeVisible();

  await page.getByTestId("settings-add-wallet").click();
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(CREATED_12);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await expect(page.getByTestId("onboarding-done")).toBeVisible();
  const secondId = await page.evaluate(() => {
    const added = window.__dwFixture!.world.wallets.at(-1) as { scanHeight?: number | null; summary: { walletId: string } };
    added.scanHeight = null;
    return added.summary.walletId;
  });
  expect(secondId).toBe(firstId);
  await page.getByTestId("onboarding-open").click();

  await expectUnsettled(page, "The wallet is still starting", "The wallet is still starting");
  expect(errors).toEqual([]);
});
