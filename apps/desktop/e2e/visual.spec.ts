import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, SCREENS, show, THEMES, type Theme } from "./helpers";
import { showSettingsView, type SettingsView } from "./settings";
import { showWalletView, type WalletView } from "./wallet";

// Visual baselines of the four gate screens, U-03's wallet screens, U-04's settings and U-05's Tools tabs in light and dark
// (Chromium). Opt-in with DW_VISUAL=1: pixels are only comparable inside the pinned Playwright image (scripts/e2e-docker.sh and the
// CI job's container), where fonts, Chromium and its software rasteriser are fixed.
test.skip(process.env.DW_VISUAL !== "1", "visual baselines run in the Playwright image only (DW_VISUAL=1)");
test.skip(({ browserName }) => browserName !== "chromium", "baselines are Chromium's");

for (const theme of THEMES) {
  for (const screen of SCREENS) {
    test(`visual ${screen} ${theme}`, async ({ page }) => {
      await show(page, screen, theme);
      await expect(page).toHaveScreenshot(`${theme}-${screen}.png`, { animations: "disabled", caret: "hide" });
    });
  }
}

// U-02: onboarding, the lock screen's wait, Forgot passphrase and Show Recovery Phrase, on fixture
// phrases only. The verify step's positions and chips are drawn at random, so they are masked.
const ONBOARDING: Record<string, (page: Page) => Promise<void>> = {
  welcome: async (page) => {
    await page.getByTestId("onboarding-welcome").waitFor();
  },
  passphrase: async (page) => {
    await page.getByTestId("onboarding-create").click();
    await page.getByTestId("onboarding-passphrase-new").fill("Fixture pass 42!");
  },
  phrase: async (page) => {
    await page.getByTestId("onboarding-create").click();
    await page.getByTestId("onboarding-passphrase-new").fill("Fixture pass 42!");
    await page.getByTestId("onboarding-passphrase-repeat").fill("Fixture pass 42!");
    await page.getByTestId("onboarding-passphrase-submit").click();
    await page.getByTestId("onboarding-phrase-words").waitFor();
  },
  verify: async (page) => {
    await ONBOARDING.phrase(page);
    await page.getByTestId("onboarding-written-down").check();
    await page.getByTestId("onboarding-phrase-continue").click();
    await page.getByTestId("onboarding-verifyPhrase").waitFor();
  },
  restore: async (page) => {
    await page.getByTestId("onboarding-restore").click();
    await page.getByTestId("restore-input").fill("enjoy congxx lay");
    await page.getByTestId("restore-problem").waitFor();
  },
};

const LATER: Record<string, { scenario: string; go: (page: Page) => Promise<void> }> = {
  "lock-wait": { scenario: "locked-throttled", go: (page) => page.getByTestId("lock-error").getByText("try again").waitFor() },
  forgot: {
    scenario: "locked-two-wallets",
    go: async (page) => {
      await page.getByTestId("lock-forgot").click();
      await page.getByTestId("forgot-wallet").first().click();
      await page.getByTestId("forgot-consequences").waitFor();
    },
  },
  "recovery-phrase": {
    scenario: "funded",
    go: async (page) => {
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "recoveryPhrase" }));
      await page.getByTestId("recovery-passphrase-field").fill("demo");
      await page.keyboard.press("Enter");
      await page.getByTestId("recovery-words").waitFor();
    },
  },
};

async function shoot(page: Page, name: string, theme: Theme) {
  await fontsReady(page);
  await expect(page).toHaveScreenshot(`${theme}-${name}.png`, {
    animations: "disabled",
    caret: "hide",
    mask: [page.locator(".verify-slots, .verify-chips")],
  });
}

for (const theme of THEMES) {
  for (const [name, go] of Object.entries(ONBOARDING)) {
    test(`visual onboarding-${name} ${theme}`, async ({ page }) => {
      await page.goto(fixtureUrl("no-wallet", theme));
      await go(page);
      await shoot(page, `onboarding-${name}`, theme);
    });
  }
  for (const [name, { scenario, go }] of Object.entries(LATER)) {
    test(`visual ${name} ${theme}`, async ({ page }) => {
      await page.goto(fixtureUrl(scenario, theme));
      await page.locator("[data-testid=hero], [data-testid=lock-screen]").first().waitFor();
      await go(page);
      await shoot(page, name, theme);
    });
  }
}

const WALLET: WalletView[] = ["pay", "payConfirm", "payResult", "receive", "activityTable"];

for (const theme of THEMES) {
  for (const view of WALLET) {
    test(`visual ${view} ${theme}`, async ({ page }) => {
      await showWalletView(page, view, theme);
      await expect(page).toHaveScreenshot(`${theme}-${view}.png`, { animations: "disabled", caret: "hide" });
    });
  }
}

// U-05: each Tools tab on the funded fixture; the console after a command, PSBT with one loaded.
const TOOLS: Record<string, (page: Page) => Promise<void>> = {
  information: (page) => page.getByTestId("tools-information").waitFor(),
  console: async (page) => {
    await page.getByTestId("console-line").fill("getblockcount");
    await page.getByTestId("console-run").click();
    await page.getByTestId("console-entry-reply").waitFor();
  },
  peers: (page) => page.getByTestId("peers-table").waitFor(),
  repair: (page) => page.getByTestId("tools-repair").waitFor(),
  addressBook: (page) => page.getByTestId("address-book-table").waitFor(),
  signVerify: (page) => page.getByTestId("tools-signVerify").waitFor(),
  psbt: async (page) => {
    await page.getByTestId("psbt-paste").fill(Buffer.from("psbt\xffunsigned", "latin1").toString("base64"));
    await page.getByTestId("psbt-load-text").click();
    await page.getByTestId("psbt-lines").waitFor();
  },
  coinSelection: (page) => page.getByTestId("coin-table").waitFor(),
};

for (const theme of THEMES) {
  for (const [tab, ready] of Object.entries(TOOLS)) {
    test(`visual tools-${tab} ${theme}`, async ({ page }) => {
      await page.goto(fixtureUrl("funded", theme));
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate((t) => window.__dwFixture!.shellCommand({ kind: "tools", tab: t as never }), tab);
      await ready(page);
      await shoot(page, `tools-${tab}`, theme);
    });
  }
}

const SETTINGS: SettingsView[] = ["general", "security", "wallets", "backups", "about", "exportSheet"];

for (const theme of THEMES) {
  for (const view of SETTINGS) {
    test(`visual settings-${view} ${theme}`, async ({ page }) => {
      await showSettingsView(page, view, theme);
      await expect(page).toHaveScreenshot(`${theme}-settings-${view}.png`, { animations: "disabled", caret: "hide" });
    });
  }
}

// DP2-07: Contacts with Hidden open and a contact selected.
for (const theme of THEMES) {
  test(`visual contacts ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    await page.getByTestId("contacts-group-hidden").getByRole("button").click();
    await page.getByTestId("contact-row").filter({ hasText: "Carol" }).click();
    await page.getByTestId("contact-detail").waitFor();
    // DP2-09's pane, read in full: the private details and the (empty) activity.
    await page.getByTestId("contact-private").waitFor();
    await page.getByTestId("contact-activity-empty").waitFor();
    // No row under the pointer.
    await page.mouse.move(0, 0);
    await shoot(page, "contacts", theme);
  });
}

// DP2-08: Add Contact with a search hit's card (Send), and My QR.
for (const theme of THEMES) {
  test(`visual add-contact ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    await page.getByTestId("contacts-add").click();
    await page.getByRole("searchbox", { name: "Search username" }).fill("ma");
    await page.getByTestId("add-contact-hit").filter({ hasText: "Marta" }).click();
    await page.getByTestId("add-contact-send").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "add-contact", theme);
  });

  test(`visual add-contact-my-qr ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
    await page.getByTestId("contacts-add").click();
    await page.getByTestId("add-contact-tabs-myQr").click();
    await page.getByTestId("add-contact-qr").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "add-contact-my-qr", theme);
  });
}

// DP2-09: the bell's popover over Home, and the Notifications page.
for (const theme of THEMES) {
  test(`visual bell ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("hero").waitFor();
    await page.getByTestId("toolbar-bell").click();
    await page.getByTestId("bell-new").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "bell", theme);
  });

  test(`visual notifications ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
    await page.getByTestId("notifications-earlier").waitFor();
    await shoot(page, "notifications", theme);
  });
}
