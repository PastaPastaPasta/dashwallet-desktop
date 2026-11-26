import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations, settleAnimations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// QT-036, QT-139: Settings ▸ General ▸ Decimal places, the fewest decimals a compact amount shows.
// It pads only: an amount with more digits shows them all (DEC-1058).

async function openGeneral(page: Page, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-general").click();
  await expect(page.getByTestId("settings-display")).toBeVisible();
}

async function shootHome(page: Page, path: string) {
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("hero-amount").waitFor();
  await page.locator(".tx-row-amount").first().waitFor();
  await fontsReady(page);
  await settleAnimations(page);
  await page.screenshot({ path });
}

for (const theme of THEMES) {
  test(`QT-036 Decimal places 2 and 8 on Home, ${theme}`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await openGeneral(page, theme);
    await expect(page.getByTestId("settings-decimals-2")).toHaveAttribute("aria-pressed", "true");
    await fontsReady(page);
    await page.getByTestId("settings-display").screenshot({ path: artifactPath(info, `${theme}-settings-decimals-2.png`) });
    await shootHome(page, artifactPath(info, `${theme}-home-decimals-2.png`));

    await page.getByTestId("sidebar-settings").click();
    await page.getByTestId("settings-tabs-general").click();
    await page.getByTestId("settings-decimals-8").click();
    await expect(page.getByTestId("settings-decimals-8")).toHaveAttribute("aria-pressed", "true");
    expect(await page.evaluate(() => window.__dwFixture!.world.settings.ui.decimalDigits)).toBe(8);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("settings-display").screenshot({ path: artifactPath(info, `${theme}-settings-decimals-8.png`) });
    await shootHome(page, artifactPath(info, `${theme}-home-decimals-8.png`));
    // Every compact amount on Home now has eight decimals; none lost a digit.
    for (const text of await page.locator(".tx-row-amount .amount-number").allTextContents()) expect(text).toMatch(/\.\d{8}$/);
    expect(errors).toEqual([]);
  });
}

test("QT-036 the choices follow the unit, and units without them have no row", async ({ page }) => {
  await openGeneral(page, "light");
  await page.getByTestId("settings-unit-milliDash").click();
  await expect(page.getByTestId("settings-decimals-5")).toBeVisible();
  await expect(page.getByTestId("settings-decimals-6")).toHaveCount(0);
  for (const unit of ["microDash", "duffs"]) {
    await page.getByTestId(`settings-unit-${unit}`).click();
    await expect(page.getByTestId("settings-decimals-2")).toHaveCount(0);
  }
  await page.getByTestId("settings-unit-dash").click();
  await expect(page.getByTestId("settings-decimals-8")).toBeVisible();
});

test("QT-036 seven choices fit at the window's minimum width", async ({ page }) => {
  await page.setViewportSize({ width: 920, height: 720 });
  await openGeneral(page, "light");
  const row = page.getByTestId("settings-decimals-8").locator("xpath=ancestor::div[contains(@class,'settings-row')]");
  const fits = await row.evaluate((el) => el.scrollWidth <= el.clientWidth);
  expect(fits).toBe(true);
  const card = await page.getByTestId("settings-display").boundingBox();
  const last = await page.getByTestId("settings-decimals-8").boundingBox();
  expect(last!.x + last!.width).toBeLessThanOrEqual(card!.x + card!.width);
});
