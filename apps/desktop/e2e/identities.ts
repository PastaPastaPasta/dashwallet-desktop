import { expect, type Page } from "@playwright/test";
import { fixtureUrl, trackPageErrors, type Theme } from "./helpers";

// DP6-01: Settings ▸ Advanced ▸ Identities, for identities.spec.ts and the visual baselines.

/** Opens Settings ▸ Advanced on `scenario`. */
export async function openIdentities(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await toAdvanced(page);
  return errors;
}

export async function toAdvanced(page: Page) {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-advanced").click();
  await expect(page.getByTestId("identities-page")).toBeVisible();
}

