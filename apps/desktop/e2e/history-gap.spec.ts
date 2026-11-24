import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// #172: an open wallet whose history starts before the block data this install keeps. Home says its
// older payments aren't shown and offers Repair's reset; "Not now" hides it until the next start.
// DW_ROUTE_SHOTS=<dir> (passed through by scripts/e2e-docker.sh) also writes each screenshot there.

const shots = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  return errors;
}

async function shoot(page: Page, name: string) {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await fontsReady(page);
  await page.screenshot({ path: `${shots}/${name}.png` });
}

for (const theme of THEMES) {
  test(`Home says a wallet's older payments aren't shown yet (${theme})`, async ({ page }) => {
    const errors = await open(page, "history-gap", theme);
    const notice = page.getByTestId("history-gap-notice");
    await expect(notice).toBeVisible();
    await expect(notice.getByRole("heading")).toHaveText("Older payments aren't shown yet");
    await expect(page.getByTestId("history-gap-text")).toHaveText(/^The history of “[^”]+” starts before block 1100000, where this app's block data begins\./);
    await expect(page.getByTestId("history-gap-action")).toHaveText(
      "Reset chain data and resync downloads it again from the earliest wallet's birth. Your wallets, keys and settings are kept. It takes several minutes, and you can't send until it finishes.",
    );
    await expect(page.getByTestId("history-gap-reset")).toHaveText("Reset chain data and resync…");
    await expect(page.getByTestId("history-gap-not-now")).toHaveText("Not now");
    await shoot(page, `home-history-gap-${theme}`);
    expect(errors).toEqual([]);
  });

  test(`two wallets are counted (${theme})`, async ({ page }) => {
    await open(page, "history-gap-two-wallets", theme);
    await expect(page.getByTestId("history-gap-text")).toHaveText(/^The history of 2 wallets starts before block 1100000/);
    await shoot(page, `home-history-gap-two-${theme}`);
  });
}

test("the reset asks as Repair does, and the notice goes once it has run", async ({ page }) => {
  const errors = await open(page, "history-gap", "light");
  await page.getByTestId("history-gap-reset").click();
  const sheet = page.getByTestId("history-gap-confirm");
  await expect(sheet).toContainText("This deletes the downloaded block headers and filters and syncs them again. Your wallets, keys and settings are kept. Continue?");
  await page.getByTestId("history-gap-confirm-cancel").click();
  await expect(sheet).toHaveCount(0);
  await expect(page.getByTestId("history-gap-notice")).toBeVisible();
  await page.getByTestId("history-gap-reset").click();
  await page.getByTestId("history-gap-confirm-ok").click();
  await expect(page.getByTestId("history-gap-notice")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a reset refused while mixing says so, and sync runs again", async ({ page }) => {
  const errors = await open(page, "history-gap-mixing", "light");
  await page.getByTestId("history-gap-reset").click();
  await page.getByTestId("history-gap-confirm-ok").click();
  await expect(page.getByTestId("history-gap-error")).toHaveText("CoinJoin is mixing. Stop mixing first, then try again.");
  await expect(page.getByTestId("history-gap-notice")).toBeVisible();
  await expect(page.getByTestId("history-gap-reset")).toBeEnabled();
  await expect(page.getByTestId("status-bar")).toContainText("8 peers");
  expect(errors).toEqual([]);
});

test("a reset refused while a mixing round settles says to wait, and sync runs again", async ({ page }) => {
  const errors = await open(page, "history-gap-settling", "light");
  await page.getByTestId("history-gap-reset").click();
  await page.getByTestId("history-gap-confirm-ok").click();
  await expect(page.getByTestId("history-gap-error")).toHaveText(
    "A CoinJoin round that just finished is still settling, so chain data can't be reset yet. This takes up to an hour. Try again then.",
  );
  await expect(page.getByTestId("history-gap-notice")).toBeVisible();
  await expect(page.getByTestId("history-gap-reset")).toBeEnabled();
  await expect(page.getByTestId("status-bar")).toContainText("8 peers");
  expect(errors).toEqual([]);
});

test("Not now hides the notice until the app starts again", async ({ page }) => {
  await open(page, "history-gap", "light");
  await page.getByTestId("history-gap-not-now").click();
  await expect(page.getByTestId("history-gap-notice")).toHaveCount(0);
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("hero").waitFor();
  await expect(page.getByTestId("history-gap-notice")).toHaveCount(0);
  await page.reload();
  await page.getByTestId("hero").waitFor();
  await expect(page.getByTestId("history-gap-notice")).toBeVisible();
});

test("a wallet with no gap shows no notice", async ({ page }) => {
  await open(page, "funded", "light");
  await expect(page.getByTestId("hero")).toBeVisible();
  await expect(page.getByTestId("history-gap-notice")).toHaveCount(0);
});
