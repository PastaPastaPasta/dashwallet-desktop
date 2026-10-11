import { expect, test } from "@playwright/test";
import { artifactPath, SCREENS, show, THEMES } from "./helpers";

// U3: the four gate screens in light and dark.
for (const theme of THEMES) {
  for (const screen of SCREENS) {
    test(`${screen} ${theme}`, async ({ page }, info) => {
      await show(page, screen, theme);
      await page.screenshot({ path: artifactPath(info, `${theme}-${screen}.png`) });
      await expect(page.locator("body")).not.toContainText("could not start");
    });
  }
}

test("narrow window stacks the breakdown strip", async ({ page }, info) => {
  await page.setViewportSize({ width: 920, height: 600 });
  await show(page, "overview", "light");
  const cells = page.locator(".breakdown-cell");
  const [a, b] = [await cells.nth(0).boundingBox(), await cells.nth(1).boundingBox()];
  expect(b!.y).toBeGreaterThan(a!.y + a!.height - 1);
  await page.screenshot({ path: artifactPath(info, "light-overview-920x600.png") });
});

test("discreet mode hides amounts and history", async ({ page }) => {
  await show(page, "overview", "light");
  await page.getByTestId("toolbar-discreet").click();
  await expect(page.getByTestId("hero-amount")).toContainText("#.##");
  await expect(page.getByTestId("history-hidden")).toBeVisible();
});
