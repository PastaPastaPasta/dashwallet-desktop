import type { Page } from "@playwright/test";
import { fixtureUrl, fontsReady, type Theme } from "./helpers";

// The U-03 screens in a fixed state, for axe (wallet.spec.ts) and the visual baselines.

/** A testnet P2PKH address the fixture's classify_address accepts. */
export const PAYEE = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";

export const WALLET_VIEWS = ["pay", "payAdvanced", "payConfirm", "payResult", "receive", "requestSheet", "activityTable"] as const;
export type WalletView = (typeof WALLET_VIEWS)[number];

/** Opens the `funded` wallet and shows `view`. */
export async function showWalletView(page: Page, view: WalletView, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  if (view.startsWith("pay")) {
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    await page.getByTestId("pay-label").fill("Alice");
    await page.getByTestId("pay-fee-line").waitFor();
  }
  if (view === "payAdvanced") {
    await page.getByTestId("pay-advanced").locator("summary").click();
    await page.getByTestId("coin-choice-selected").check();
    await page.getByTestId("coin-table").locator("tbody tr").first().locator("input").check();
    await page.getByTestId("coin-summary").waitFor();
  }
  if (view === "payConfirm" || view === "payResult") {
    const review = page.getByTestId("pay-continue");
    await review.and(page.locator(":enabled")).waitFor();
    await review.click();
    await page.getByTestId("pay-confirm-passphrase").waitFor();
  }
  if (view === "payResult") {
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await page.getByTestId("pay-confirm-send").click();
    await page.getByTestId("pay-result-sent").waitFor();
  }
  if (view === "receive" || view === "requestSheet") {
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("receive-qr").waitFor();
  }
  if (view === "requestSheet") {
    await page.getByTestId("request-label").fill("Rent");
    await page.getByTestId("request-amount").fill("0.25");
    await page.getByTestId("request-message").fill("October");
    const create = page.getByTestId("request-create");
    await create.and(page.locator(":enabled")).waitFor();
    await create.click();
    await page.getByTestId("request-qr").waitFor();
  }
  if (view === "activityTable") {
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-mode-table").click();
    await page.getByTestId("tx-table").waitFor();
  }
  await fontsReady(page);
}
