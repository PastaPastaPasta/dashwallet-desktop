import { expect, test, type Locator, type Page } from "@playwright/test";
import { HOME } from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, trackPageErrors } from "./helpers";

// The registration wizard and Change keys draw their choices as `.mg-choices` radio groups in a
// centred sheet. As fieldsets with a legend they had the shape of issue 108 (WebKitGTK sized the
// legend twice, and the first click's blur moved the sheet under the pointer), so they have no
// fieldset or legend. One click on a choice selects it, and the sheet keeps its place when the
// focused control loses focus. Tall enough that the sheet isn't at its height limit, so a doubled
// legend would show as movement.
test.use({ viewport: { width: 1280, height: 1200 } });

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

async function steady(page: Page, sheet: Locator) {
  const before = await sheet.boundingBox();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  const after = await sheet.boundingBox();
  expect(after!.height).toBeCloseTo(before!.height, 0);
  expect(after!.y).toBeCloseTo(before!.y, 0);
}

test("the wizard's Type step: the sheet stays put on blur, and one click picks a choice", async ({ page }) => {
  const errors = await open(page, "reg");
  await page.getByTestId("mn-set-up").click();
  const sheet = page.getByTestId("reg-sheet");
  await expect(page.getByTestId("reg-step-type")).toBeVisible();
  await expect(sheet.locator("fieldset, legend")).toHaveCount(0);
  await steady(page, sheet);
  await page.getByTestId("reg-kind-evonode").click({ position: { x: 60, y: 10 } });
  await expect(page.getByTestId("reg-kind-evonode").locator("input")).toBeChecked();
  expect(errors).toEqual([]);
});

test("Change keys: the sheet stays put on blur, and one click picks a choice", async ({ page }) => {
  const errors = await open(page, "mn-list");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await page.getByTestId("mn-change-keys").click();
  const sheet = page.getByTestId("mn-change-keys-sheet");
  await expect(sheet).toBeVisible();
  await expect(sheet.locator("fieldset, legend")).toHaveCount(0);
  await steady(page, sheet);
  await page.getByTestId("mn-change-operator-keychain").click({ position: { x: 60, y: 10 } });
  await expect(page.getByTestId("mn-change-operator-keychain").locator("input")).toBeChecked();
  expect(errors).toEqual([]);
});
