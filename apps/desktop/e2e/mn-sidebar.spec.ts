import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, trackPageErrors } from "./helpers";

// MG-10 §3.6: the Masternodes item's Always / Automatic / Never setting. Automatic shows the item
// for a masternode of this wallet's or a tracked one, a collateral coin, or a ProTx operation not
// yet settled; shared sessions join in S5.

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function choose(page: Page, choice: "on" | "auto" | "off") {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-masternodes-item").selectOption(choice);
}

const item = (page: Page) => page.getByTestId("sidebar-masternodes");

test("Automatic is the default and hides the item with nothing there", async ({ page }) => {
  const errors = await open(page, "funded");
  await expect(item(page)).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await expect(page.getByTestId("settings-masternodes-item")).toHaveValue("auto");
  expect(errors).toEqual([]);
});

test("Automatic shows the item for the wallet's own masternodes, and Never hides it", async ({ page }) => {
  const errors = await open(page, "mn-list");
  await choose(page, "auto");
  await expect(item(page)).toBeVisible();
  await choose(page, "off");
  await expect(item(page)).toHaveCount(0);
  await choose(page, "auto");
  await expect(item(page)).toBeVisible();
  expect(errors).toEqual([]);
});

test("Automatic shows the item for a collateral coin alone", async ({ page }) => {
  const errors = await open(page, "reg");
  await choose(page, "auto");
  await expect(item(page)).toBeVisible();
  await item(page).click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  expect(errors).toEqual([]);
});

test("Automatic shows the item for a saved registration, and the badge counts it", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await choose(page, "auto");
  await expect(item(page)).toBeVisible();
  await expect(page.getByTestId("sidebar-masternodes-badge")).toHaveText("1");
  expect(errors).toEqual([]);
});

test("Always shows the item with nothing there", async ({ page }) => {
  const errors = await open(page, "funded");
  await choose(page, "on");
  await page.getByTestId("settings-governance-turn-on").click();
  await expect(item(page)).toBeVisible();
  expect(errors).toEqual([]);
});
