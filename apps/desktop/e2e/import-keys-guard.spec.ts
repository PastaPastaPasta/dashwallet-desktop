import { expect, test, type Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Settings ▸ Backups ▸ Import keys takes an HD seed or an extended private key: like the
// recovery-phrase flows (onboarding.spec.ts, D27), the field is not on the page until the shell
// has answered that the window is kept out of screen captures.

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
const secretShown = (page: Page) => page.evaluate(() => window.__dwFixture!.world.shellSync?.secretShown);

async function backups(page: Page, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-tabs-backups").click();
  await page.getByTestId("settings-import-keys").waitFor();
  return errors;
}

async function axeClean(page: Page) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
}

const field = (page: Page) => page.getByTestId("settings-keys-text");

for (const theme of THEMES) {
  test(`Import keys shows no key field until the window is protected (${theme})`, async ({ page }) => {
    const errors = await backups(page, theme);
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("shell_sync"));
    await page.getByTestId("settings-import-keys").click();
    await expect(page.getByTestId("protection-pending")).toBeVisible();
    await expect(field(page)).toHaveCount(0);
    await expect(page.getByTestId("settings-keys-hdSeed")).toHaveCount(0);
    await expect.poll(() => secretShown(page)).toBe(true);
    await axeClean(page);

    await release.evaluate((r) => r());
    await expect(field(page)).toBeVisible();
    await expect(page.getByTestId("protection-pending")).toHaveCount(0);
    await axeClean(page);

    await page.keyboard.press("Escape");
    await expect(page.getByTestId("settings-import-keys-sheet")).toHaveCount(0);
    await expect.poll(() => secretShown(page)).toBe(false);
    expect(errors).toEqual([]);
  });

  test(`Import keys stays closed when the window cannot be protected, and Try again opens it (${theme})`, async ({ page }) => {
    const errors = await backups(page, theme);
    await page.evaluate(() => window.__dwFixture!.fail("shell_sync", "none"));
    await page.getByTestId("settings-import-keys").click();
    await expect(page.getByTestId("protection-failed")).toContainText("so it will not take a seed or private key");
    await expect(field(page)).toHaveCount(0);
    await axeClean(page);

    // Still refused: still no field.
    await page.getByTestId("protection-retry").click();
    await expect(page.getByTestId("protection-failed")).toBeVisible();
    await expect(field(page)).toHaveCount(0);

    await page.evaluate(() => window.__dwFixture!.fail("shell_sync", null));
    await page.getByTestId("protection-retry").click();
    await expect(field(page)).toBeVisible();
    await expect(page.getByTestId("protection-failed")).toHaveCount(0);
    await axeClean(page);
    expect(errors).toEqual([]);
  });
}
