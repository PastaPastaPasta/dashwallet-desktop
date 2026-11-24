import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// A confirmed shared masternode in My masternodes (MG-10b S6): the operator's share line only when the
// registration carries one, and a standby's one valid send, never one whose penalty is unknown.

const SHOTS = process.env.DW_SHOTS;
const SH_ROW = `mn-row-${"ec".repeat(32)}`;

const open = async (page: Page, scenario: string, theme: Theme) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await page.getByTestId(SH_ROW).click();
  await expect(page.getByTestId("sh-detail")).toBeVisible();
  return errors;
};

for (const theme of THEMES) {
  test(`shared masternode detail ${theme}: the operator's share line follows the registration`, async ({ page }, info) => {
    const errors = await open(page, "sh-detail", theme);
    await expect(page.getByTestId("sh-detail-operator-share")).toContainText("5%");
    await expect(page.getByTestId("mn-fact-operator-share")).toContainText("5%");
    await expect(page.getByTestId("mn-fact-operator-share")).not.toContainText("of each payout");
    await expect(page.getByTestId("sh-detail").getByRole("button", { name: /send/i })).toHaveText(["Send now and pay the penalty"]);
    if (SHOTS && info.project.name === "chromium") await page.screenshot({ path: `${SHOTS}/after-sh-detail-full-${theme}.png`, fullPage: true });
    await page.goto(fixtureUrl("sh-detail-no-operator-share", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("sidebar-masternodes").click();
    await page.getByTestId(SH_ROW).click();
    await expect(page.getByTestId("sh-detail")).toBeVisible();
    await expect(page.getByTestId("sh-detail-operator-share")).toHaveCount(0);
    await expect(page.getByTestId("mn-fact-operator-share")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`shared masternode detail ${theme}: an immediate standby with an unknown penalty is listed, never offered to send`, async ({ page }) => {
    const errors = await open(page, "sh-detail-penalty-unknown", theme);
    await expect(page.getByTestId("sh-detail")).toContainText("you pay the early-exit penalty");
    await expect(page.getByTestId("sh-detail").getByRole("button", { name: /send|penalty/i })).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
