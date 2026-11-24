import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, trackPageErrors } from "./helpers";

// Issue #116: in the username vote sheet, one click on a choice selects it. The choices were a flex
// fieldset with a visible legend in a vertically centred sheet that focuses its close button, the
// same three ingredients as Restore options in #108, where WebKitGTK sized the legend twice and the
// first click's blur moved the content under the pointer between mousedown and mouseup.

async function openSheet(page: Page) {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("names", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await page.getByTestId("masternodes-tabs-names").click();
  await page.getByTestId("name-vote").click();
  const sheet = page.getByTestId("name-vote-sheet");
  await expect(sheet).toBeVisible();
  await expect(sheet.locator(".sheet-close")).toBeFocused();
  return { errors, sheet };
}

// Tall enough that the sheet isn't at its height limit, so a doubled legend would show as movement.
test.use({ viewport: { width: 1280, height: 1200 } });

const choices = (page: Page) => page.getByRole("group", { name: "Your vote" });

test("the choices are a labelled group with no legend, as tall as their content", async ({ page }) => {
  const { sheet } = await openSheet(page);
  await expect(choices(page)).toBeVisible();
  await expect(sheet.locator("legend")).toHaveCount(0);
  await expect(sheet.locator("fieldset")).toHaveCount(0);
  // A fieldset's doubled legend made its box taller than the title and rows in it.
  const [top, bottom] = await choices(page).evaluate((group) => {
    const box = group.getBoundingClientRect();
    return [group.firstElementChild!.getBoundingClientRect().top - box.top, box.bottom - group.lastElementChild!.getBoundingClientRect().bottom];
  });
  expect(Math.abs(top)).toBeLessThan(1);
  expect(Math.abs(bottom)).toBeLessThan(1);
});

test("the sheet keeps its height when the close button loses focus", async ({ page }) => {
  const { sheet } = await openSheet(page);
  const before = await sheet.boundingBox();
  await sheet.locator(".sheet-close").evaluate((el: HTMLElement) => el.blur());
  const after = await sheet.boundingBox();
  expect(after!.height).toBeCloseTo(before!.height, 0);
  expect(after!.y).toBeCloseTo(before!.y, 0);
});

test("one click on a choice's words selects it while the close button has focus", async ({ page }) => {
  const { errors } = await openSheet(page);
  await page.getByTestId("name-choice-lock").click({ position: { x: 60, y: 10 } });
  await expect(page.getByTestId("name-choice-lock").locator("input")).toBeChecked();
  await expect(page.getByTestId("name-vote-send")).toHaveText("Vote Lock");
  expect(errors).toEqual([]);
});

test("one click on a choice's radio selects it while the close button has focus", async ({ page }) => {
  await openSheet(page);
  await page.getByTestId("name-choice-abstain").locator("input").click();
  await expect(page.getByTestId("name-choice-abstain").locator("input")).toBeChecked();
});

test("every choice selects with one click", async ({ page }) => {
  await openSheet(page);
  const options = choices(page).locator("label");
  const count = await options.count();
  expect(count).toBeGreaterThan(2);
  for (let i = 0; i < count; i++) {
    await options.nth(i).click({ position: { x: 60, y: 10 } });
    await expect(options.nth(i).locator("input")).toBeChecked();
  }
});

test("the keyboard reaches the choices and arrows move between them", async ({ page }) => {
  await openSheet(page);
  const radios = choices(page).getByRole("radio");
  // From the focused close button, Tab reaches the first choice.
  await page.keyboard.press("Tab");
  await expect(radios.first()).toBeFocused();
  await page.keyboard.press("Space");
  await expect(radios.first()).toBeChecked();
  await page.keyboard.press("ArrowDown");
  await expect(radios.nth(1)).toBeChecked();
});
