// A payment refused while the engine cannot read the wallet's payment records (`evidence_unreadable`):
// Send and Tools ▸ PSBT say why, and the wallet's balance still shows.

import { expect, test } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, openToolsTab, THEMES, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

const UNSIGNED = Buffer.from("psbt\xffunsigned", "latin1").toString("base64");
const COPY = "Dash Wallet can't read some of this wallet's payment records, so sending is paused. Try again in a moment, or restart Dash Wallet.";

for (const theme of THEMES) {
  test(`a payment refused for unreadable payment records says why (${theme})`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("evidence-unreadable", theme));
    await page.getByTestId("toolbar").waitFor();
    await expect(page.getByTestId("hero")).toBeVisible();
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("pay-confirm")).toBeVisible();
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByText(COPY)).toBeVisible();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `evidence-unreadable-send-${theme}.png`) });
    expect(errors).toEqual([]);
  });
}

for (const theme of THEMES) {
  test(`signing a PSBT refused for unreadable payment records says why (${theme})`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("evidence-unreadable", theme));
    await page.getByTestId("toolbar").waitFor();
    await openToolsTab(page, "psbt");
    await page.getByTestId("psbt-paste").fill(UNSIGNED);
    await page.getByTestId("psbt-load-text").click();
    await expect(page.getByTestId("psbt-lines")).toContainText("Transaction has 2 unsigned inputs.");
    await page.getByTestId("psbt-sign").click();
    const sheet = page.getByTestId("grant-sheet");
    await sheet.locator("input[type=password]").fill("demo");
    await sheet.locator("input[type=password]").press("Enter");
    await expect(page.getByTestId("psbt-error")).toHaveText(COPY);
    await expect(page.getByTestId("psbt-broadcast")).toBeDisabled();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `evidence-unreadable-psbt-${theme}.png`) });
    expect(errors).toEqual([]);
  });
}
