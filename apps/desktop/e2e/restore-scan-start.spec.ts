import { expect, test, type Page } from "@playwright/test";
import { MAIN_PHRASE } from "../src/fixture/vault";
import { fixtureUrl, trackPageErrors } from "./helpers";

// Issue #108: on Restore options, one click on a scan start selects it, and its field shows.
// In WebKitGTK a flex fieldset's legend was sized twice when the step mounted; the first click's
// blur of the passphrase field relaid the step 14px lower, so its mouseup missed the option.

async function restoreOptions(page: Page) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("no-wallet", "light"));
  await page.getByTestId("onboarding-restore").click();
  const input = page.getByTestId("restore-input");
  await expect(input).toBeFocused();
  await page.keyboard.type(MAIN_PHRASE);
  await expect(page.getByTestId("restore-valid")).toHaveText("The recovery phrase is valid.");
  await page.getByTestId("restore-continue").click();
  await expect(page.getByTestId("onboarding-restoreOptions")).toBeVisible();
  await expect(page.getByTestId("restore-bip39")).toBeFocused();
  await expect(page.getByTestId("restore-scan-genesis")).toBeChecked();
  return errors;
}

const scanStart = (page: Page) => page.getByRole("group", { name: "Scan the blockchain from" });
const option = (page: Page, text: string) => scanStart(page).locator("label").filter({ hasText: text });

test("the scan start is a labelled group with no legend, and the step is as tall as its content", async ({ page }) => {
  await restoreOptions(page);
  await expect(scanStart(page)).toBeVisible();
  await expect(page.locator("[data-testid=onboarding-restoreOptions] legend")).toHaveCount(0);
  const gap = await page.getByTestId("onboarding-restoreOptions").evaluate((body) => body.getBoundingClientRect().bottom - body.lastElementChild!.getBoundingClientRect().bottom);
  expect(Math.abs(gap)).toBeLessThan(1);
});

test("one click on the words of A block height selects it and shows its field", async ({ page }) => {
  const errors = await restoreOptions(page);
  await option(page, "A block height").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-scan-height")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toBeVisible();
  expect(errors).toEqual([]);
});

test("one click on the radio of A block height selects it", async ({ page }) => {
  await restoreOptions(page);
  await page.getByTestId("restore-scan-height").click();
  await expect(page.getByTestId("restore-scan-height")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toBeVisible();
});

test("one click after typing a BIP39 passphrase selects A block height", async ({ page }) => {
  await restoreOptions(page);
  await page.keyboard.type("twenty-fifth");
  await option(page, "A block height").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-scan-height")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toBeVisible();
});

test("every scan start selects with one click", async ({ page }) => {
  await restoreOptions(page);
  await option(page, "The date the wallet was created").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-scan-date")).toBeChecked();
  await expect(page.getByTestId("restore-date")).toBeVisible();
  await option(page, "A block height").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-scan-height")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toBeVisible();
  await expect(page.getByTestId("restore-date")).toHaveCount(0);
  await option(page, "The beginning").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-scan-genesis")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toHaveCount(0);
});

test("the keyboard reaches the scan start group and arrows select A block height", async ({ page, browserName }) => {
  await restoreOptions(page);
  // WebKit's Tab skips checkboxes and radios by default, as Safari does.
  if (browserName === "webkit") {
    await page.getByTestId("restore-scan-genesis").focus();
  } else {
    await page.keyboard.press("Tab");
    await expect(page.getByTestId("restore-core-compat")).toBeFocused();
    await page.keyboard.press("Tab");
  }
  await expect(page.getByTestId("restore-scan-genesis")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByTestId("restore-scan-date")).toBeChecked();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByTestId("restore-scan-height")).toBeChecked();
  await expect(page.getByTestId("restore-height")).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("restore-height")).toBeFocused();
});

// #686: a restore from the first block says it can take long and may hold a watch-only
// wallet's sends; the date option refuses a missing date instead of scanning from block 0.
test("a restore from the first block warns that it is slow, and a missing date is refused", async ({ page }) => {
  const errors = await restoreOptions(page);
  const slow = page.getByTestId("restore-first-block-slow");
  await expect(slow).toHaveText(
    "This can take a long time. If this recovery phrase belongs to a watch-only wallet already in Dash Wallet, sending from that wallet is unavailable until the scan finishes.",
  );
  await page.getByTestId("restore-scan-height").check();
  await page.getByTestId("restore-height").fill("1200");
  await expect(slow).toHaveCount(0);

  await page.getByTestId("restore-scan-date").check();
  await expect(page.getByRole("alert")).toHaveText("Enter a valid wallet creation date, or choose The beginning.");
  await expect(page.getByTestId("restore-finish")).toBeDisabled();
  await expect(slow).toHaveCount(0);
  await page.getByTestId("restore-date").fill("2023-02-28");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByTestId("restore-finish")).toBeEnabled();
  await expect(page.getByTestId("restore-scan-start")).toContainText("The scan starts at block");

  await option(page, "The beginning").click({ position: { x: 60, y: 8 } });
  await expect(page.getByTestId("restore-finish")).toBeEnabled();
  await expect(slow).toBeVisible();
  expect(errors).toEqual([]);
});
