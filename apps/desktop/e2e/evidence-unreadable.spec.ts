// A payment refused while the engine cannot read the wallet's payment records (`evidence_unreadable`):
// Send says why, and the wallet's balance still shows.

import { expect, test } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

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
