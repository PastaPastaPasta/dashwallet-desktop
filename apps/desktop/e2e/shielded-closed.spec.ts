import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// The upgrade's move of the old Shielded records stopped (`shielded.store_closed`): Home says Shielded
// couldn't open and why, the rest of the wallet works, and the notice offers Restart where a start may
// open it and Report a problem where the user needs support. A restart that opens Shielded removes it.
// DW_ROUTE_SHOTS=<dir> (passed through by scripts/e2e-docker.sh) also writes each screenshot there.

const shots = process.env.DW_ROUTE_SHOTS;
const world = (page: Page) => page.evaluate(() => window.__dwFixture!.world);
const KEEP = "keep your wallet data as it is: support may need it.";

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  return errors;
}

/** The wallet behind the lock screen a restart leaves (it reloads the page with the vault locked), unlocked.
 * Waits for that lock screen itself: the page before the reload still shows Home. */
async function unlockAfterRestart(page: Page) {
  await page.getByTestId("lock-screen").waitFor();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("lock-field").press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}

async function shoot(page: Page, name: string) {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await fontsReady(page);
  await page.screenshot({ path: `${shots}/${name}.png` });
}

const REASONS = [
  { scenario: "shielded-closed-in-use", reason: "in_use", text: "Another program, possibly another copy of DashPay Desktop, is using your Shielded data", restart: true, report: false },
  { scenario: "shielded-closed-failed", reason: "failed", text: "DashPay Desktop couldn't move your Shielded data to this version, so it didn't open Shielded. The rest", restart: true, report: true },
  { scenario: "shielded-closed-damaged", reason: "damaged", text: "DashPay Desktop couldn't read your Shielded data from before the upgrade, so it didn't open Shielded and changed nothing in that data.", restart: false, report: true },
  { scenario: "shielded-closed-no-copy", reason: "no_upgrade_copy", text: "DashPay Desktop has no backup of your Shielded data from before the upgrade", restart: false, report: true },
];

for (const theme of THEMES) {
  for (const r of REASONS) {
    test(`Home says Shielded couldn't open: ${r.reason} (${theme})`, async ({ page }) => {
      const errors = await open(page, r.scenario, theme);
      const notice = page.getByTestId("shielded-closed-notice");
      await expect(notice).toBeVisible();
      await expect(notice).toHaveAttribute("data-reason", r.reason);
      await expect(notice.getByRole("heading")).toHaveText("Shielded couldn't open");
      await expect(page.getByTestId("shielded-closed-text")).toContainText(r.text);
      await expect(page.getByTestId("shielded-closed-text")).toContainText("The rest of your wallet works as usual.");
      if (r.report) await expect(page.getByTestId("shielded-closed-text")).toContainText(KEEP);
      await expect(page.getByTestId("shielded-closed-restart")).toHaveCount(r.restart ? 1 : 0);
      await expect(page.getByTestId("shielded-closed-report")).toHaveCount(r.report ? 1 : 0);
      await expect(page.getByTestId("shielded-closed-issues-hint")).toHaveCount(r.report ? 1 : 0);
      if (r.report) await expect(page.getByTestId("shielded-closed-report")).toHaveText("Report a problem");
      await expect(page.getByTestId("breakdown-core")).toBeVisible();
      await shoot(page, `home-shielded-closed-${r.reason}-${theme}`);
      expect(errors).toEqual([]);
    });
  }
}

test("Restart starts again, and the notice goes once Shielded opens", async ({ page }) => {
  const errors = await open(page, "shielded-closed-in-use", "light");
  await page.getByTestId("shielded-closed-restart").click();
  await unlockAfterRestart(page);
  await page.getByTestId("hero").waitFor();
  expect((await world(page)).appExits).toEqual(["restart"]);
  await expect(page.getByTestId("shielded-closed-notice")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("Report a problem opens the issues page", async ({ page }) => {
  await open(page, "shielded-closed-damaged", "light");
  await page.getByTestId("shielded-closed-report").click();
  await expect.poll(async () => (await world(page)).linksOpened).toEqual(["issues"]);
});

test("an open Shielded shows no notice", async ({ page }) => {
  await open(page, "funded", "light");
  await expect(page.getByTestId("breakdown-shielded")).toBeVisible();
  await expect(page.getByTestId("shielded-closed-notice")).toHaveCount(0);
});
