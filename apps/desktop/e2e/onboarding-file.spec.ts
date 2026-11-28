import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_WALLET_ID, SECOND_WALLET_ID } from "../src/fixture/scenarios";
import { FIXTURE_FILES, FIXTURE_RESTORE_BACKUP, FIXTURE_RESTORE_BACKUP_PASSPHRASE } from "../src/fixture/settings";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Restore from a file during setup on the fixture backend: Choose File… (or a typed path), the
// backup's passphrase, then the new vault's, then every wallet of the backup. DW_ROUTE_SHOTS=<dir>
// (scripts/e2e-docker.sh) also writes each step's screenshot there.

const shots = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("onboarding-welcome").waitFor();
  return errors;
}

async function shot(page: Page, name: string, theme: Theme) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
}

const walletIds = (page: Page) => page.evaluate(() => window.__dwFixture!.world.wallets.map((w) => w.summary.walletId));

for (const theme of THEMES)
  test(`restore a backup on a new install ${theme}: choose the file, its passphrase, then the vault's`, async ({ page }) => {
    const errors = await open(page, "no-wallet-backup-file", theme);
    await shot(page, "welcome", theme);
    await page.getByTestId("onboarding-restore-file").click();
    await expect(page.getByTestId("onboarding-restoreFile")).toBeVisible();
    await expect(page.getByTestId("restore-file-choose")).toBeFocused();
    await shot(page, "file-path", theme);
    await page.getByTestId("restore-file-choose").click();
    await expect(page.getByTestId("restore-file-kind")).toHaveText("A DashPay Desktop backup of 2 wallets.");
    await expect(page.getByTestId("restore-file-chosen")).toContainText("Main and Savings.dwbackup");
    await expect(page.getByTestId("restore-file-passphrase")).toBeFocused();
    await page.getByTestId("restore-file-restore").click();
    await expect(page.getByTestId("restore-file-error")).toHaveText("Enter the file's passphrase.");
    await page.getByTestId("restore-file-passphrase").fill(FIXTURE_RESTORE_BACKUP_PASSPHRASE);
    await expect(page.getByTestId("restore-file-error")).toHaveCount(0);
    await shot(page, "file-details", theme);
    await page.getByTestId("restore-file-restore").click();
    // No vault yet: its passphrase comes last, as a phrase restore's does.
    await expect(page.getByTestId("onboarding-choosePassphrase")).toBeVisible();
    expect(await walletIds(page)).toEqual([]);
    await page.getByTestId("onboarding-passphrase-new").fill("a long new vault passphrase");
    await page.getByTestId("onboarding-passphrase-repeat").fill("a long new vault passphrase");
    await page.getByTestId("onboarding-passphrase-submit").click();
    await expect(page.getByTestId("onboarding-done")).toContainText("2 wallets were restored.");
    expect((await walletIds(page)).toSorted()).toEqual([FIXTURE_WALLET_ID, SECOND_WALLET_ID].toSorted());
    await shot(page, "file-done", theme);
    expect(errors).toEqual([]);
  });

test("a typed path; a wrong passphrase and a file that isn't a wallet are named; Back steps out", async ({ page }) => {
  await open(page, "no-wallet-backup-file");
  await page.getByTestId("onboarding-restore-file").click();
  await page.getByTestId("restore-file-path").fill(FIXTURE_FILES.psbt);
  await page.getByTestId("restore-file-inspect").click();
  await expect(page.getByTestId("restore-file-kind")).toHaveText("A partially signed transaction, not a wallet.");
  await expect(page.getByTestId("restore-file-refusal")).toBeVisible();
  await expect(page.getByTestId("restore-file-other")).toBeFocused();
  await page.getByTestId("restore-file-other").click();
  await expect(page.getByTestId("restore-file-choose")).toBeFocused();
  await page.getByTestId("restore-file-path").fill(FIXTURE_RESTORE_BACKUP);
  await page.getByTestId("restore-file-inspect").click();
  await page.getByTestId("restore-file-passphrase").fill("wrong");
  await page.getByTestId("restore-file-restore").click();
  await page.getByTestId("onboarding-passphrase-new").fill("a long new vault passphrase");
  await page.getByTestId("onboarding-passphrase-repeat").fill("a long new vault passphrase");
  await page.getByTestId("onboarding-passphrase-submit").click();
  await expect(page.getByTestId("onboarding-restoreFile")).toBeVisible();
  await expect(page.getByTestId("restore-file-error")).toBeVisible();
  expect(await walletIds(page)).toEqual([]);
  await page.getByTestId("onboarding-back").click();
  await expect(page.getByTestId("restore-file-path")).toBeVisible();
  await page.getByTestId("onboarding-back").click();
  await expect(page.getByTestId("onboarding-welcome")).toBeVisible();
});
