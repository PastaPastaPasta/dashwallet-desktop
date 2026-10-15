import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { expect, type Page, type TestInfo } from "@playwright/test";

export type Theme = "light" | "dark";
export const THEMES: Theme[] = ["light", "dark"];
export const SCREENS = ["overview", "transactions", "txDetail", "unlock"] as const;
export type Screen = (typeof SCREENS)[number];

/** Opens the fixture app's `funded` wallet and shows `screen`. */
export async function show(page: Page, screen: Screen, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  if (screen === "transactions" || screen === "txDetail") {
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
  }
  if (screen === "txDetail") {
    await page.locator("[data-txid]").first().click();
    await page.getByTestId("tx-detail-facts").waitFor();
  }
  if (screen === "unlock") {
    await page.getByTestId("toolbar-lock").click();
    await page.getByTestId("lock-screen").waitFor();
  }
  await fontsReady(page);
}

/** The fixture build's URL for a scenario (src/fixture/index.ts `fixtureOptionsFromUrl`). */
export function fixtureUrl(scenario: string, theme: Theme, extra = ""): string {
  return `/?scenario=${scenario}&theme=${theme}${extra}`;
}

/** Collects the page's uncaught errors; the returned list fills as they happen. */
export function trackPageErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  return errors;
}

/** Waits for web fonts, so text metrics and screenshots are final. */
export async function fontsReady(page: Page) {
  await page.evaluate(() => document.fonts.ready);
}

const repo = resolve(import.meta.dirname, "../../..");

/** Writes into docs/screenshots/gate when DW_WRITE_SCREENSHOTS=1, else into the test output. */
export function artifactPath(info: TestInfo, name: string): string {
  if (process.env.DW_WRITE_SCREENSHOTS === "1") {
    const p = resolve(repo, "docs/screenshots/gate", `playwright-${info.project.name}`, name);
    mkdirSync(dirname(p), { recursive: true });
    return p;
  }
  return info.outputPath(name);
}

export function writeJson(path: string, value: unknown) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value, null, 2));
}

/** The passphrase sheet of a pending grant, answered (the fixture's passphrase by default). */
export async function authorize(page: Page, passphrase = "demo") {
  const sheet = page.getByTestId("grant-sheet");
  await sheet.locator("input[type=password]").fill(passphrase);
  await sheet.locator("input[type=password]").press("Enter");
  await expect(sheet).toHaveCount(0);
}
