import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, SCREENS, show, THEMES, type Theme } from "./helpers";
import { showWalletView, type WalletView } from "./wallet";

// Visual baselines of the four gate screens and U-03's wallet screens in light and dark
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
