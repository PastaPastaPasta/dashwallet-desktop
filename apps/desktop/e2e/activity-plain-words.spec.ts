import { expect, test } from "@playwright/test";
import { fixtureUrl } from "./helpers";

test("history uses plain titles, a negative self-send fee, and expandable mixing groups", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light"));
  await page.getByTestId("toolbar").waitFor();

  await expect(page.getByTestId("page-home")).toContainText("Username @dwdemo2611");
  await page.getByTestId("sidebar-activity").click();
  await expect(page.getByTestId("page-activity")).toBeVisible();
  await expect(page.getByText("Username @dwdemo2611", { exact: true })).toBeVisible();
  await expect(page.getByText("Asset lock", { exact: true })).toHaveCount(0);

  const self = page.getByRole("button", { name: /Sent to yourself/ }).first();
  await expect(self).toContainText("-0.00000226");
  await self.click();
  await expect(page.getByTestId("tx-detail-title")).toHaveText("Sent to yourself");
  await expect(page.getByTestId("tx-detail-amount")).toContainText("-0.00000226");
  await page.keyboard.press("Escape");

  const username = page.getByRole("button", { name: /Username @dwdemo2611/ }).first();
  await username.click();
  await expect(page.getByTestId("tx-detail-title")).toHaveText("Username @dwdemo2611");
  await expect(page.getByTestId("tx-detail-status")).toContainText("Confirmed");
  await expect(page.getByTestId("tx-detail-status")).not.toContainText("InstantSend");
  await page.keyboard.press("Escape");

  const mixing = page.locator(".tx-mixing-group").first();
  await expect(mixing).toContainText("Mixing · 12 transactions");
  await expect(mixing).toHaveAttribute("aria-expanded", "false");
  await mixing.click();
  await expect(mixing).toHaveAttribute("aria-expanded", "true");
  const members = page.locator(`#${await mixing.getAttribute("aria-controls")}`);
  await expect(members.locator(".tx-row")).toHaveCount(12);
  expect(await page.locator(".tx-row-title", { hasText: "Mixing" }).count()).toBeGreaterThan(13);
});
