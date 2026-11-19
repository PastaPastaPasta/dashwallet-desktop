import { expect, test, type Page } from "@playwright/test";
import type { World } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// PK-03a on the fixture backend (src/fixture/updates.ts): the one check at launch, About's Updates
// line with Release notes, the status bar's chip, and Settings ▸ General ▸ Privacy's "Check for
// updates" (off by default, absent inside Flatpak).

const world = <T,>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function settings(page: Page, tab: "about" | "general") {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId(`settings-tabs-${tab}`).click();
  await fontsReady(page);
}

const CASES = [
  { scenario: "update-available", line: "Version 0.9.0 is available", chip: true, requests: 1 },
  { scenario: "update-current", line: "Dash Wallet is up to date", chip: false, requests: 1 },
  { scenario: "update-error", line: "Could not check for updates", chip: false, requests: 1 },
  { scenario: "update-off", line: "Not checked: the update check is off", chip: false, requests: 0 },
  { scenario: "update-flatpak", line: "Updates come through Flatpak", chip: false, requests: 0 },
] as const;

for (const theme of THEMES) {
  for (const c of CASES) {
    test(`${c.scenario} ${theme}: About, the status bar and the requests made`, async ({ page }) => {
      const errors = await open(page, c.scenario, theme);
      await settings(page, "about");
      const row = page.getByTestId("settings-about-updates");
      await expect(row).toContainText(c.line);
      await expect(page.getByTestId("settings-about-release-notes")).toHaveCount(c.chip ? 1 : 0);
      await expect(page.getByTestId("status-update")).toHaveCount(c.chip ? 1 : 0);
      expect(await world(page, (w) => w.updates.requests)).toBe(c.requests);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test(`release notes open from About and the chip ${theme}`, async ({ page }) => {
    await open(page, "update-available", theme);
    await expect(page.getByTestId("status-update")).toHaveText("Update available");
    await page.getByTestId("status-update").click();
    await expect.poll(() => world(page, (w) => w.updates.opened)).toEqual(["0.9.0"]);
    await settings(page, "about");
    await page.getByTestId("settings-about-release-notes").click();
    await expect.poll(() => world(page, (w) => w.updates.opened)).toEqual(["0.9.0", "0.9.0"]);
    await expect(page.getByTestId("settings-about-release-notes-problem")).toHaveCount(0);
  });

  test(`Privacy: turning the check on asks once, off forgets ${theme}`, async ({ page }) => {
    await open(page, "update-off", theme);
    await settings(page, "general");
    const box = page.getByTestId("settings-update-check");
    await expect(box).not.toBeChecked();
    await box.check();
    await expect(page.getByTestId("status-update")).toBeVisible();
    expect(await world(page, (w) => w.updates.requests)).toBe(1);
    await box.uncheck();
    await expect(page.getByTestId("status-update")).toHaveCount(0);
    expect(await world(page, (w) => w.updates.requests)).toBe(1);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });
}

test("Flatpak: Privacy offers no update check", async ({ page }) => {
  await open(page, "update-flatpak", "light");
  await settings(page, "general");
  await page.getByTestId("settings-privacy").waitFor();
  await expect(page.getByTestId("settings-update-check")).toHaveCount(0);
});
