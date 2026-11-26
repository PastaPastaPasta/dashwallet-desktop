import { expect, test } from "@playwright/test";
import { seriousAxeViolations, settleAnimations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";
import { showSettingsView } from "./settings";

// QT-018, QT-153: dash-qt's CoinJoin information as "How CoinJoin works" on the CoinJoin page, and
// the license on Settings ▸ About (UX-SPEC §4.18).

for (const theme of THEMES) {
  test(`QT-153 How CoinJoin works opens from the CoinJoin page and closes, ${theme}`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("funded", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
    await page.getByTestId("coinjoin-status-title").waitFor();
    await fontsReady(page);
    await settleAnimations(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-coinjoin-page.png`) });
    await page.getByTestId("coinjoin-explainer-open").click();
    const sheet = page.getByTestId("coinjoin-explainer");
    await expect(sheet).toBeVisible();
    await expect(sheet).toContainText("CoinJoin makes it harder to trace where your Dash came from.");
    await expect(sheet).toContainText("Other payments can use coins that aren't mixed.");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await fontsReady(page);
    await sheet.screenshot({ path: artifactPath(info, `${theme}-coinjoin-explainer.png`) });
    await page.getByTestId("coinjoin-explainer-close").click();
    await expect(sheet).toHaveCount(0);
    await expect(page.getByTestId("coinjoin-explainer-open")).toBeFocused();
    // WebKit on macOS does not focus a button it clicks: focus still comes back to it.
    const opener = page.getByTestId("coinjoin-explainer-open");
    await opener.blur();
    await opener.dispatchEvent("click");
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(opener).toBeFocused();
    expect(errors).toEqual([]);
  });

  test(`QT-153 Settings ▸ About shows the MIT License, ${theme}`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await showSettingsView(page, "about", theme);
    const card = page.getByTestId("settings-about-license");
    await expect(card).toContainText("Dash Wallet is free software, released under the MIT License.");
    const text = page.getByTestId("settings-about-license-text");
    await expect(text).toContainText("Copyright (c) Dash Wallet Desktop contributors");
    await expect(text).toContainText('THE SOFTWARE IS PROVIDED "AS IS"');
    expect(await seriousAxeViolations(page)).toEqual([]);
    await settleAnimations(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-settings-about.png`) });
    await card.scrollIntoViewIfNeeded();
    await card.screenshot({ path: artifactPath(info, `${theme}-settings-about-license.png`) });
    expect(errors).toEqual([]);
  });
}
