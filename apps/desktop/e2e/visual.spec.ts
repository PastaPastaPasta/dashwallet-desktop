import { expect, test, type Page } from "@playwright/test";
import { CARDS, CHIP_TEXT, CHIPS } from "./dashpay";
import { authorize, fixtureUrl, fontsReady, SCREENS, show, THEMES, type Theme } from "./helpers";
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

// DP1-08: every Home card state and identity chip (element shots), the Join DashPay page's
// readiness states, the voting info and the FAQ.
const SHOT = { animations: "disabled", caret: "hide" } as const;

const JOIN: Record<string, { scenario: string; ready: string }> = {
  join: { scenario: "dashpay-no-identity", ready: "join-cost" },
  "join-shortfall": { scenario: "empty", ready: "join-shortfall" },
  "join-waiting": { scenario: "dashpay-waiting-for-sync", ready: "join-waiting" },
  "join-watch-only": { scenario: "dashpay-watch-only", ready: "join-watch-only" },
  "join-voting": { scenario: "dashpay-contest-pending", ready: "dashpay-card" },
};

// DP1-09: the registration wizard's two steps and a registration's screens on the Join page.
const REGISTRATION: Record<string, { scenario: string; reach: (page: Page) => Promise<unknown> }> = {
  name: {
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await page.getByTestId("registration-name-field").fill("alice");
      await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
    },
  },
  confirm: {
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await page.getByTestId("registration-name-field").fill("alice-77");
      await expect(page.getByTestId("registration-next")).toBeEnabled();
      await page.getByTestId("registration-next").click();
      await page.getByTestId("registration-fee").waitFor();
    },
  },
  progress: { scenario: "dashpay-registering", reach: (page) => page.locator("[data-testid=registration-screen][data-kind=progress]").waitFor() },
  paused: { scenario: "dashpay-registration-paused", reach: (page) => page.locator("[data-testid=registration-screen][data-kind=paused]").waitFor() },
  retry: { scenario: "dashpay-registering-paused", reach: (page) => page.locator("[data-testid=registration-screen][data-kind=retry]").waitFor() },
  repair: { scenario: "dashpay-registration-needs-finish", reach: (page) => page.locator("[data-testid=registration-screen][data-kind=repair]").waitFor() },
  stopped: { scenario: "dashpay-registration-stopped", reach: (page) => page.locator("[data-testid=registration-screen][data-kind=stopped]").waitFor() },
};

async function openJoin(page: Page, scenario: string, theme: Theme) {
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
}

for (const theme of THEMES) {
  for (const [state, { scenario, extra }] of Object.entries(CARDS)) {
    test(`visual dashpay-card-${state} ${theme}`, async ({ page }) => {
      await page.goto(fixtureUrl(scenario, theme, extra));
      const card = page.getByTestId("dashpay-card");
      await card.waitFor();
      await fontsReady(page);
      await expect(card).toHaveScreenshot(`${theme}-dashpay-card-${state}.png`, SHOT);
    });
  }
  for (const [kind, scenario] of Object.entries(CHIPS)) {
    test(`visual dashpay-chip-${kind} ${theme}`, async ({ page }) => {
      await page.goto(fixtureUrl(scenario, theme));
      const chip = page.getByTestId("toolbar-chip");
      await chip.waitFor();
      // The registration's username, the main name or the lack of one, once read.
      if (CHIP_TEXT[kind]) await expect(chip).toContainText(CHIP_TEXT[kind]);
      await fontsReady(page);
      await expect(chip).toHaveScreenshot(`${theme}-dashpay-chip-${kind}.png`, SHOT);
    });
  }
  for (const [name, { scenario, ready }] of Object.entries(JOIN)) {
    test(`visual dashpay-${name} ${theme}`, async ({ page }) => {
      await openJoin(page, scenario, theme);
      await page.getByTestId(ready).waitFor();
      await shoot(page, `dashpay-${name}`, theme);
    });
  }
  test(`visual dashpay-voting-info ${theme}`, async ({ page }) => {
    await openJoin(page, "dashpay-no-identity", theme);
    await page.getByTestId("join-voting-info").click();
    await page.getByTestId("voting-info").waitFor();
    await shoot(page, "dashpay-voting-info", theme);
  });
  test(`visual dashpay-faq ${theme}`, async ({ page }) => {
    await openJoin(page, "dashpay-no-identity", theme);
    await page.getByTestId("join-faq").click();
    await page.getByTestId("dashpay-faq-item").first().locator("summary").click();
    // No question under the pointer.
    await page.mouse.move(0, 0);
    await shoot(page, "dashpay-faq", theme);
  });
  for (const [name, { scenario, reach }] of Object.entries(REGISTRATION)) {
    test(`visual dashpay-registration-${name} ${theme}`, async ({ page }) => {
      await openJoin(page, scenario, theme);
      await reach(page);
      await shoot(page, `dashpay-registration-${name}`, theme);
    });
  }
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
    // DP3-07: Home's frequent strip lands after the hero; the popover covers part of it.
    await page.getByTestId("home-frequent").waitFor();
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

// DP3-06: Pay ▸ To a contact: the chooser (frequent strip and picker), the form with a payee, and
// "Accept and pay"'s confirm sheet for a contact request.
async function openPayContact(page: Page, theme: Theme) {
  await page.goto(fixtureUrl("dashpay-registered", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("pay-frequent-chip").first().waitFor();
  await page.getByTestId("pay-picker-contacts").waitFor();
}

async function payContactForm(page: Page, name: string) {
  await page.getByTestId("pay-picker-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) }).click();
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-fee-line").waitFor();
  await page.getByTestId("pay-continue").and(page.locator(":enabled")).waitFor();
}

for (const theme of THEMES) {
  test(`visual pay-contact ${theme}`, async ({ page }) => {
    await openPayContact(page, theme);
    await page.mouse.move(0, 0);
    await shoot(page, "pay-contact", theme);
  });

  test(`visual pay-contact-form ${theme}`, async ({ page }) => {
    await openPayContact(page, theme);
    await payContactForm(page, "Bob Builder");
    await page.mouse.move(0, 0);
    await shoot(page, "pay-contact-form", theme);
  });

  test(`visual pay-contact-accept ${theme}`, async ({ page }) => {
    await openPayContact(page, theme);
    await payContactForm(page, "Grace Hopper");
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("pay-confirm-passphrase").waitFor();
    await shoot(page, "pay-contact-accept", theme);
  });
}

// DP3-07: Home with the frequent strip and contact rows, Activity's list with them, and the
// detail naming the contact (View contact), on `dashpay-history-contacts`.
async function openHistoryContacts(page: Page, theme: Theme) {
  await page.goto(fixtureUrl("dashpay-history-contacts", theme));
  await page.getByTestId("home-frequent-chip").first().waitFor();
}

for (const theme of THEMES) {
  test(`visual home-contacts ${theme}`, async ({ page }) => {
    await openHistoryContacts(page, theme);
    await page.mouse.move(0, 0);
    await shoot(page, "home-contacts", theme);
  });

  test(`visual activity-contacts ${theme}`, async ({ page }) => {
    await openHistoryContacts(page, theme);
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "activity-contacts", theme);
  });

  test(`visual tx-detail-contact ${theme}`, async ({ page }) => {
    await openHistoryContacts(page, theme);
    await page.locator(".tx-row").filter({ hasText: "Coffee refund" }).click();
    await page.getByTestId("tx-detail-view-contact").waitFor();
    await shoot(page, "tx-detail-contact", theme);
  });
}

// DP1-10: My Profile's states (the design's state inventory) and the top-up sheet's steps.
async function openProfile(page: Page, scenario: string, theme: Theme) {
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
}

const PROFILE: Record<string, { scenario: string; ready: string }> = {
  registered: { scenario: "dashpay-registered", ready: "profile-outcomes" },
  "no-profile": { scenario: "dashpay-no-identity", ready: "dashpay-card" },
  "no-username": { scenario: "dashpay-no-username", ready: "profile-choose-name" },
  "credits-low": { scenario: "dashpay-credits-low", ready: "profile-credits-estimate" },
  "credits-empty": { scenario: "dashpay-low-credits", ready: "profile-credits-estimate" },
  "credits-unknown": { scenario: "dashpay-credits-unknown", ready: "profile-credits" },
  "top-up-pending": { scenario: "dashpay-top-up-pending", ready: "profile-pending-finish" },
  "top-up-unknown": { scenario: "dashpay-top-up-unknown", ready: "profile-pending-arrived" },
  "top-up-stuck": { scenario: "dashpay-top-up-stuck", ready: "profile-stuck" },
  "contest-open": { scenario: "dashpay-contest-pending", ready: "profile-tally" },
  "contest-instant": { scenario: "dashpay-contest-no-temporary", ready: "profile-instant" },
  "contest-won": { scenario: "dashpay-contest-won", ready: "profile-tally" },
  "contest-lost": { scenario: "dashpay-contest-lost", ready: "profile-tally" },
  "contest-locked": { scenario: "dashpay-contest-locked", ready: "profile-tally" },
};

for (const theme of THEMES) {
  for (const [name, { scenario, ready }] of Object.entries(PROFILE)) {
    test(`visual profile-${name} ${theme}`, async ({ page }) => {
      await openProfile(page, scenario, theme);
      await page.getByTestId(ready).waitFor();
      // The outcome rows land after the page (D12).
      if (scenario === "dashpay-registered") await page.getByTestId("profile-outcomes").waitFor();
      await page.mouse.move(0, 0);
      await shoot(page, `profile-${name}`, theme);
    });
  }

  test(`visual profile-top-up ${theme}`, async ({ page }) => {
    await openProfile(page, "dashpay-registered", theme);
    await page.getByTestId("profile-top-up").click();
    await page.getByTestId("topup-next").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "profile-top-up-amount", theme);
    await page.getByTestId("topup-next").click();
    await page.getByTestId("topup-quote").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "profile-top-up-confirm", theme);
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await page.getByTestId("topup-done").waitFor();
    await page.mouse.move(0, 0);
    await shoot(page, "profile-top-up-done", theme);
  });
}
