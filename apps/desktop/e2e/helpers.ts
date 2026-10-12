import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import type { Page, TestInfo } from "@playwright/test";

export type Theme = "light" | "dark";
export const THEMES: Theme[] = ["light", "dark"];
export const SCREENS = ["overview", "transactions", "txDetail", "unlock"] as const;
export type Screen = (typeof SCREENS)[number];

/** Opens the fixture app and shows `screen`. */
export async function show(page: Page, screen: Screen, theme: Theme, query = "") {
  await page.goto(`/?theme=${theme}${query}`);
  await page.getByTestId("hero").waitFor();
  if (screen === "transactions" || screen === "txDetail") {
    await page.getByTestId("sidebar-transactions").click();
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
