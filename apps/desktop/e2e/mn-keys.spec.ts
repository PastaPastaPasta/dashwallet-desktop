import { expect, test, type Page } from "@playwright/test";
import { TEST_VECTOR_WIF } from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Masternode keys (IOS-083, MG-10 §1.5): opened from Masternodes ▸ More, segments per role, "Show
// more" paging, rows matched to known masternodes, and a reveal behind the passphrase that is
// dropped on hide.

const open = async (page: Page, theme: Theme) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("mn-list", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  await page.getByTestId("mn-tab-more").click();
  await page.getByRole("menuitem", { name: "Masternode keys…" }).click();
  await expect(page.getByTestId("mn-keys")).toBeVisible();
};

test("Masternode keys lists each role, pages, and reveals one secret behind the passphrase", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "light");
  const sheet = page.getByTestId("mn-keys");
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await expect(sheet.getByTestId("mn-key-0")).toContainText("Keypair 1");
  await expect(sheet.getByTestId("mn-key-1-match")).toHaveText("Matched to Office node · 203.0.113.9:19999");
  await expect(sheet.getByTestId("mn-key-2-match")).toHaveText("Not matched to a known masternode");

  await sheet.getByTestId("mn-keys-role-operator").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(20);
  await sheet.getByTestId("mn-keys-more").click();
  await expect(sheet.getByTestId("mn-keys-list").locator("li")).toHaveCount(25);
  await expect(sheet.getByTestId("mn-keys-more")).toHaveCount(0);

  await sheet.getByTestId("mn-keys-role-owner").click();
  await sheet.getByTestId("mn-key-0-show").click();
  await expect(sheet.getByTestId("mn-key-0-address")).toBeVisible();
  await expect(sheet.getByTestId("mn-key-0-hidden")).toBeVisible();
  await sheet.getByTestId("mn-key-0-reveal").click();
  const field = sheet.getByTestId("mn-key-0-passphrase-form").getByLabel("Passphrase");
  await field.fill("demo");
  await field.press("Enter");
  await expect(sheet.getByTestId("mn-key-secret-wif")).toContainText(TEST_VECTOR_WIF);
  await sheet.getByTestId("mn-key-0-hide-secret").click();
  await expect(sheet.getByTestId("mn-key-secret-wif")).toHaveCount(0);
  await expect(sheet.getByTestId("mn-key-0-hidden")).toBeVisible();
  expect(errors).toEqual([]);
});

for (const theme of THEMES) {
  test(`Masternode keys, ${theme}`, async ({ page }, info) => {
    await open(page, theme);
    const sheet = page.getByTestId("mn-keys");
    await sheet.getByTestId("mn-keys-role-operator").click();
    await sheet.getByTestId("mn-key-0-show").click();
    await expect(sheet.getByTestId("mn-key-0-public-key")).toBeVisible();
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `mn-keys-operator-${theme}.png`) });
  });
}
