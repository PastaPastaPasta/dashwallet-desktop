import { expect, test } from "@playwright/test";
import { SCREENS, show, THEMES } from "./helpers";
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

const WALLET: WalletView[] = ["pay", "payConfirm", "payResult", "receive", "activityTable"];

for (const theme of THEMES) {
  for (const view of WALLET) {
    test(`visual ${view} ${theme}`, async ({ page }) => {
      await showWalletView(page, view, theme);
      await expect(page).toHaveScreenshot(`${theme}-${view}.png`, { animations: "disabled", caret: "hide" });
    });
  }
}
