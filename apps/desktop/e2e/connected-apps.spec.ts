import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, trackPageErrors, type Theme } from "./helpers";
import { toAdvanced } from "./identities";

// Settings ▸ Advanced ▸ Connected apps & Disconnect (DashPay App Connect):
// lists approved apps for the current identity (claimed name marked not verified, App ID,
// return host, key scopes, funding limit, expiry, approval date); Disconnect shows the live
// Platform credit fee, requires the passphrase, removes the row on success, and keeps it with
// a plain error on failure; mainnet refuses it.

async function openConnectedApps(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await toAdvanced(page);
  await page.getByTestId("connected-apps-card").waitFor();
  return errors;
}

test("lists connected apps with unverified name, App ID, return host, scopes, limit, expiry and approval date", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps");
  const card = page.getByTestId("connected-apps-card");
  await expect(card.getByRole("heading", { name: "Connected apps" })).toBeVisible();
  const rows = card.getByTestId("connected-app-row");
  await expect(rows).toHaveCount(2);

  const yappr = rows.nth(0);
  await expect(yappr.getByTestId("connected-app-claim")).toHaveText("“Yappr” (name given by the app, not verified)");
  await expect(yappr.getByTestId("connected-app-id")).toHaveText(/^[0-9a-f]{64}$/);
  await expect(yappr.getByTestId("connected-app-returns-to")).toContainText("yappr.example");
  await expect(yappr.getByTestId("connected-app-limit")).toContainText("0.05 tDASH");
  await expect(yappr.getByTestId("connected-app-expiry")).not.toBeEmpty();
  await expect(yappr.getByTestId("connected-app-approved")).not.toBeEmpty();
  await expect(yappr.getByTestId("connected-app-key")).toHaveCount(2);
  await expect(yappr.getByTestId("connected-app-key").nth(0)).toContainText("Key 6");
  await expect(yappr.getByTestId("connected-app-key").nth(0)).toContainText("owner and admins can add contracts");
  await expect(yappr.getByTestId("connected-app-key").nth(1)).toContainText("Key 7");
  await expect(yappr.getByTestId("connected-app-key").nth(1)).toContainText("DashPay profile");

  const forum = rows.nth(1);
  await expect(forum.getByTestId("connected-app-claim")).toHaveText("“Dash Forum” (name given by the app, not verified)");
  await expect(forum.getByTestId("connected-app-returns-to")).toHaveCount(0);
  await expect(forum.getByTestId("connected-app-key")).toHaveCount(1);

  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("shows empty state when no apps are connected", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps-empty");
  const empty = page.getByTestId("connected-apps-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("No connected apps.");
  await expect(empty).toContainText("Apps you sign in to with DashPay Connect show here.");
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("Disconnect shows the fee, requires the passphrase, and removes the app on success", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps");
  const rows = page.getByTestId("connected-app-row");
  await expect(rows).toHaveCount(2);

  await rows.nth(0).getByTestId("connected-app-disconnect").click();
  const sheet = page.getByTestId("disconnect-sheet");
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole("heading", { name: "Disconnect this app?" })).toBeVisible();
  await expect(sheet.getByTestId("disconnect-claim")).toHaveText("“Yappr” (name given by the app, not verified)");
  await expect(sheet.getByTestId("disconnect-fee")).toContainText("Disabling these keys costs about");
  await expect(sheet.getByTestId("disconnect-fee")).toContainText("in credits.");

  await sheet.getByTestId("disconnect-confirm").click();
  const grantSheet = page.getByTestId("grant-sheet");
  await expect(grantSheet).toBeVisible();
  await expect(grantSheet).toContainText("Enter your passphrase to disconnect this app.");

  // Wrong passphrase shows an error and keeps the sheet open.
  await grantSheet.getByTestId("grant-sheet-field").fill("wrong-passphrase");
  await grantSheet.getByTestId("grant-sheet-submit").click();
  await expect(grantSheet.getByTestId("grant-sheet-error")).not.toBeEmpty();

  // Correct passphrase disconnects and removes the row.
  await authorize(page);
  await expect(page.getByTestId("disconnect-sheet")).toHaveCount(0);
  await expect(page.getByTestId("connected-apps-done")).toHaveText("Disconnected “Yappr”.");
  await expect(page.getByTestId("connected-app-row")).toHaveCount(1);
  await expect(page.getByTestId("connected-app-row").nth(0).getByTestId("connected-app-claim")).toContainText("Dash Forum");

  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("Disconnect failure shows a plain error and keeps the row in the list", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps");
  const rows = page.getByTestId("connected-app-row");
  await rows.nth(0).getByTestId("connected-app-disconnect").click();
  await expect(page.getByTestId("disconnect-fee")).toContainText("in credits.");

  await page.evaluate(() => {
    window.__dwFixture!.world.disconnectOutcome = "notConfirmed";
  });
  await page.getByTestId("disconnect-confirm").click();
  await authorize(page);

  await expect(page.getByTestId("disconnect-error")).toContainText(
    "Dash Platform didn't confirm the change, so the keys may already be disabled. Try again: Dash Wallet checks first, so you don't pay twice.",
  );
  await page.getByTestId("disconnect-cancel").click();
  await expect(page.getByTestId("disconnect-sheet")).toHaveCount(0);
  await expect(page.getByTestId("connected-app-row")).toHaveCount(2);
  expect(errors).toEqual([]);
});

test("Disconnect refuses unverified older grants with a plain error", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps");
  await page.evaluate(() => {
    window.__dwFixture!.world.disconnectOutcome = "unverifiedKey";
  });
  await page.getByTestId("connected-app-row").nth(0).getByTestId("connected-app-disconnect").click();
  await expect(page.getByTestId("disconnect-error")).toContainText(
    "Dash Wallet couldn't confirm which keys on Dash Platform belong to this app, so it didn't disable any.",
  );
  await expect(page.getByTestId("disconnect-confirm")).toBeDisabled();
  expect(errors).toEqual([]);
});

test("shows the test-networks-only notice on mainnet without listing connected apps", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(`${fixtureUrl("dashpay-connected-apps", "light")}&network=mainnet`);
  await page.getByTestId("toolbar").waitFor();
  await toAdvanced(page);
  const card = page.getByTestId("connected-apps-card");
  await expect(card.getByTestId("connected-apps-mainnet-off")).toHaveText("This version can sign in to apps on test networks only.");
  await expect(card.getByTestId("connected-app-row")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("keeps the funding-limit value and unit label consistent under non-DASH display units", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-connected-apps", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-unit-milliDash").click();
  await expect(page.getByTestId("settings-unit-milliDash")).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("settings-tabs-advanced").click();
  await page.getByTestId("connected-apps-card").waitFor();
  const limit = page.getByTestId("connected-app-row").nth(0).getByTestId("connected-app-limit");
  await expect(limit).toContainText("0.05 tDASH");
  await expect(limit).not.toContainText("mtDASH");
  expect(errors).toEqual([]);
});

test("Disconnect sheet shows only the keys included in the live quote", async ({ page }) => {
  const errors = await openConnectedApps(page, "dashpay-connected-apps");
  const yappr = page.getByTestId("connected-app-row").nth(0);
  await expect(yappr.getByTestId("connected-app-key")).toHaveCount(2);

  // Simulate Key 7 being disabled on Platform after the inventory loaded.
  await page.evaluate(() => {
    const app = window.__dwFixture!.world.connectedApps![0];
    app.keys = app.keys.filter((k) => k.keyId === 6);
  });

  await yappr.getByTestId("connected-app-disconnect").click();
  const sheet = page.getByTestId("disconnect-sheet");
  await expect(sheet.getByTestId("disconnect-fee")).toContainText("in credits.");
  await expect(sheet.getByTestId("disconnect-keys").getByTestId("connected-app-key")).toHaveCount(1);
  await expect(sheet.getByTestId("disconnect-keys")).toContainText("Key 6");
  await expect(sheet.getByTestId("disconnect-keys")).not.toContainText("Key 7");
  expect(errors).toEqual([]);
});
