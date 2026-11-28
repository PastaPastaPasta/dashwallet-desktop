import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { APP_ID, CONNECT_LINKS, deliverConnect, openConnect, quoted } from "./connect";
import { authorize, fontsReady, trackPageErrors } from "./helpers";

// "Sign in to an app" (DashPay App Connect) on the fixture backend: the sheet says who asks (the
// app's own unverified name, its App ID, where it returns to) and for what; the primary key can't be
// left out; Deny and Approve answer the request, Approve under the passphrase every time; a send
// that failed is retried; mainnet shows no Approve. dw-engine checks requests against Platform and
// dw-app keeps the request and its return link; the fixture stands in for both.

const link = (page: Page, outcome: keyof typeof CONNECT_LINKS) =>
  page.evaluate((id) => window.__dwFixture!.world.connect?.[id] ?? null, CONNECT_LINKS[outcome]);

test("the sheet shows the App ID, the app's unverified name and where it returns to; the primary key stays", async ({ page }) => {
  const errors = trackPageErrors(page);
  const sheet = await openConnect(page, "ok");
  await expect(sheet.getByRole("heading", { name: "Sign in to an app" })).toBeVisible();
  await expect(page.getByTestId("connect-claim")).toHaveText("“Yappr” (name given by the app, not verified)");
  await expect(page.getByTestId("connect-unverified")).toHaveText(
    "Dash Wallet can't check which app sent this request. Approve it only if you just started signing in on that app's own website.",
  );
  await expect(page.getByTestId("connect-app-id")).toHaveText(APP_ID);
  await expect(page.getByTestId("connect-returns-to")).toHaveText("Returns you to yappr.example");
  await expect(page.getByTestId("connect-identity")).toHaveValue(/.+/);
  await expect(page.getByTestId("connect-identity").locator("option")).toHaveText(["alice"]);
  const rows = page.getByTestId("connect-binding");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("A key for “post” documents");
  await expect(rows.nth(1)).toContainText("A key that can edit your DashPay profile");
  await expect(rows.nth(2)).toContainText("The app's owner and admins can add contracts to this group later");
  await expect(rows.nth(2).getByTestId("connect-group-owner")).toBeVisible();
  await expect(rows.nth(2).getByTestId("connect-group-tokens")).toContainText("None");
  await expect(rows.nth(0).getByTestId("connect-critical")).toHaveCount(0);
  await expect(rows.nth(1).getByTestId("connect-critical")).toHaveText("This key has critical access: within what it covers, it can do anything your identity can.");
  await expect(rows.nth(2).getByTestId("connect-critical")).toContainText("The funding limit doesn't cap tokens.");
  await expect(rows.nth(0).getByTestId("connect-token-cost")).toHaveText(
    "Saving documents with this key can also spend tokens your identity holds, if the contract charges tokens for them. The funding limit doesn't apply to tokens.",
  );
  await expect(rows.nth(1).getByTestId("connect-token-cost")).toHaveCount(0);
  await expect(rows.nth(2).getByTestId("connect-token-cost")).toHaveCount(0);
  await expect(page.getByTestId("connect-encryption")).toContainText("An encryption key for the app");
  await expect(page.getByTestId("connect-limit")).toHaveValue("0.05");
  await expect(page.getByTestId("connect-expiry")).toHaveText("In 30 days");
  await quoted(page);
  await expect(page.getByTestId("connect-limit-confirm")).toHaveText("0.05");
  const fee = await page.getByTestId("connect-fee").textContent();

  const primary = rows.nth(0).getByTestId("connect-binding-check");
  await expect(primary).toBeChecked();
  await expect(primary).toBeDisabled();
  await primary.click({ force: true });
  await expect(primary).toBeChecked();

  await rows.nth(1).getByTestId("connect-binding-check").uncheck();
  await expect(page.getByTestId("connect-fee")).not.toHaveText(fee!);
  await quoted(page);
  await page.getByTestId("connect-limit").fill("0");
  await expect(page.getByTestId("connect-limit-problem")).toHaveText("Enter an amount of Dash more than 0 and at most 5, like 0.5.");
  await expect(page.getByTestId("connect-approve")).toBeDisabled();
  await page.getByTestId("connect-limit").fill("0.123456789");
  await expect(page.getByTestId("connect-limit-problem")).not.toBeEmpty();
  await expect(page.getByTestId("connect-approve")).toBeDisabled();
  await page.getByTestId("connect-limit").fill("0.01");
  await quoted(page);
  await expect(page.getByTestId("connect-limit-confirm")).toHaveText("0.01");
  await expect(page.getByTestId("connect-approve")).toBeEnabled();
  await expect(sheet).not.toContainText(/Disconnect|Connected apps/);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("Deny answers the request; Return goes back to the app and closes the sheet", async ({ page }) => {
  await openConnect(page, "ok");
  await page.getByTestId("connect-deny").click();
  await expect(page.getByTestId("connect-denied")).toHaveText("You declined this sign-in request. Nothing was added to your identity.");
  expect((await link(page, "ok"))?.answer).toBe("declined");
  await page.getByTestId("connect-return").click();
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
  expect(await link(page, "ok")).toMatchObject({ returned: true, dismissed: true });
});

test("Approve takes the passphrase, adds the keys and offers the way back", async ({ page }) => {
  const errors = trackPageErrors(page);
  await openConnect(page, "ok");
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await expect(page.getByTestId("grant-sheet")).toBeVisible();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toHaveText("You're signed in to “Yappr”.");
  await expect(page.getByTestId("connect-return")).toHaveText("Return to yappr.example");
  expect((await link(page, "ok"))?.answer).toBe("ok");
  await page.getByTestId("connect-return").click();
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
  expect((await link(page, "ok"))?.returned).toBe(true);
  expect(errors).toEqual([]);
});

const closeSheet = (page: Page) => page.getByTestId("connect-sheet").locator(".sheet-close").click();

test("a request typed into Open URI has no way back; closing the sheet forgets the request", async ({ page }) => {
  await openConnect(page, "ok");
  await closeSheet(page);
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
  expect((await link(page, "ok"))?.dismissed).toBe(true);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "openUri" }));
  await page.getByTestId("open-uri-input").fill("dashpay://connect?v=2&fixture=1");
  await page.getByTestId("open-uri-open").click();
  await page.locator("[data-testid=connect-body][data-phase=sheet]").waitFor();
  await expect(page.getByTestId("connect-unverified")).toBeVisible();
  await expect(page.getByTestId("connect-returns-to")).toHaveCount(0);
  await page.getByTestId("connect-deny").click();
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
});

test("on mainnet there is no sheet and no Approve", async ({ page }) => {
  await openConnect(page, "mainnetOff");
  await expect(page.getByTestId("connect-mainnet-off")).toHaveText("This version can sign in to apps on test networks only.");
  await expect(page.getByTestId("connect-approve")).toHaveCount(0);
  await expect(page.getByTestId("connect-claim")).toHaveCount(0);
  await expect(page.getByTestId("connect-return")).toBeVisible();
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
});

test("a send that failed says so; Retry sending signs in", async ({ page }) => {
  await openConnect(page, "sendFailed");
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-send-failed")).toHaveText(
    "The keys were added to your identity, but the app may not have received them yet. Retry sending, or return to the app and start again.",
  );
  await expect(page.getByTestId("connect-return")).toHaveCount(0);
  await page.getByTestId("connect-retry").click();
  await expect(page.getByTestId("connect-done")).toBeVisible();
  expect((await link(page, "sendFailed"))?.answer).toBe("ok");
});

test("a refusal says why, with only Close and the way back", async ({ page }) => {
  await openConnect(page, "stale");
  await expect(page.getByTestId("connect-refusal")).toHaveText("This sign-in request has expired. Ask the app for a new one.");
  await expect(page.getByTestId("connect-approve")).toHaveCount(0);
  await expect(page.getByTestId("connect-deny")).toHaveCount(0);
  await expect(page.locator("[data-testid=connect-sheet] .sheet-actions .dash-button")).toHaveCount(2);
});

test("a request that changed on Platform is read again and the user is told", async ({ page }) => {
  await openConnect(page, "planChanged");
  await quoted(page);
  await expect(page.getByTestId("connect-binding")).toHaveCount(3);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-plan-changed")).toBeVisible();
  await expect(page.getByTestId("connect-binding")).toHaveCount(2);
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toBeVisible();
});

test("a group that changed between the sheet's read and the quote's is read again before Approve", async ({ page }) => {
  await openConnect(page, "groupChanged");
  await quoted(page);
  await expect(page.getByTestId("connect-plan-changed")).toBeVisible();
  await expect(page.getByTestId("connect-binding").nth(2).getByTestId("connect-group-admins").locator("dd")).toHaveCount(2);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toBeVisible();
});

test("a temporary Platform error when refreshing after a group mismatch keeps Try again and recovers", async ({ page }) => {
  await openConnect(page, "groupRefreshOffline");
  await expect(page.getByTestId("connect-plan-changed")).toBeVisible();
  await expect(page.getByTestId("connect-quote-error")).toHaveText("Couldn't check this request with Platform. Try again.");
  await expect(page.getByTestId("connect-quote-retry")).toBeVisible();
  await expect(page.getByTestId("connect-approve")).toBeDisabled();

  await page.getByTestId("connect-quote-retry").click();
  await expect(page.getByTestId("connect-body")).toHaveAttribute("data-phase", "sheet");
  await expect(page.getByTestId("connect-refusal")).toHaveCount(0);
  await expect(page.getByTestId("connect-quote-error")).toHaveText("Dash Platform is unavailable for this request. Make sure Dash Platform is on and connected, then try again.");
  await expect(page.getByTestId("connect-quote-retry")).toBeVisible();
  await expect(page.getByTestId("connect-approve")).toBeDisabled();

  await page.getByTestId("connect-quote-retry").click();
  await quoted(page);
  await expect(page.getByTestId("connect-quote-error")).toHaveCount(0);
  await expect(page.getByTestId("connect-quote-retry")).toHaveCount(0);
  await expect(page.getByTestId("connect-approve")).toBeEnabled();
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toBeVisible();
});

test("keys not confirmed in time leave the sheet open, and approving again finishes", async ({ page }) => {
  await openConnect(page, "notConfirmed");
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-quote-error")).toHaveText(
    "Sign-in didn't finish: Dash Platform didn't confirm the new keys in time. They may still be added; if you try again, Dash Wallet checks your identity first.",
  );
  await expect(page.getByTestId("connect-approve")).toBeEnabled();
  await expect(page.getByTestId("connect-limit")).toBeDisabled();
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toBeVisible();
});

test("a reopened sheet whose quote failed offers Try again, which quotes the same choice and turns Approve on", async ({ page }) => {
  await openConnect(page, "quoteOffline");
  await expect(page.getByTestId("connect-quote-error")).toHaveText("Dash Platform is unavailable for this request. Make sure Dash Platform is on and connected, then try again.");
  await expect(page.getByTestId("connect-limit")).toBeDisabled();
  await expect(page.getByTestId("connect-approve")).toBeDisabled();
  await page.getByTestId("connect-quote-retry").click();
  await quoted(page);
  await expect(page.getByTestId("connect-quote-error")).toHaveCount(0);
  await expect(page.getByTestId("connect-quote-retry")).toHaveCount(0);
  await expect(page.getByTestId("connect-approve")).toBeEnabled();
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-done")).toBeVisible();
});

test("a credit shortfall offers Try again, which checks the balance again", async ({ page }) => {
  await openConnect(page, "credits");
  await expect(page.getByTestId("connect-quote-error")).toContainText("more in credits");
  await expect(page.getByTestId("connect-approve")).toBeDisabled();
  await page.getByTestId("connect-quote-retry").click();
  await expect(page.getByTestId("connect-quote-error")).toContainText("more in credits");
  await expect(page.getByTestId("connect-quote-retry")).toBeVisible();
  await expect(page.getByTestId("connect-approve")).toBeDisabled();
});

test("a Deny after keys not confirmed doesn't say nothing was added", async ({ page }) => {
  await openConnect(page, "notConfirmed");
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-quote-error")).toBeVisible();
  await page.getByTestId("connect-deny").click();
  await expect(page.getByTestId("connect-denied")).toHaveText(
    "You declined this sign-in request. Keys from your earlier approval may already be on your identity, and the app may already have them.",
  );
});

test("a Deny after keys not confirmed on a typed request shows the denied state instead of closing", async ({ page }) => {
  await openConnect(page, "notConfirmed", "light", null);
  await quoted(page);
  await page.getByTestId("connect-approve").click();
  await authorize(page);
  await expect(page.getByTestId("connect-quote-error")).toBeVisible();
  await page.getByTestId("connect-deny").click();
  await expect(page.getByTestId("connect-denied")).toHaveText(
    "You declined this sign-in request. Keys from your earlier approval may already be on your identity, and the app may already have them.",
  );
  await expect(page.getByTestId("connect-return")).toHaveCount(0);
  await page.getByTestId("connect-close").click();
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
});

test("a request that arrives while locked opens once the wallet is unlocked", async ({ page }) => {
  await openConnect(page, "ok");
  await closeSheet(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await page.getByTestId("lock-field").waitFor();
  await deliverConnect(page, "ok");
  await expect(page.getByTestId("connect-sheet")).toHaveCount(0);
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("connect-claim")).toBeVisible();
});

test("a request refused because every kept request is open or being approved says so, and the open sheet stays", async ({ page }) => {
  await openConnect(page, "ok");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "rejected", reason: { kind: "connect", reason: "busy" } }]));
  await expect(page.getByText("Dash Wallet already has too many sign-in requests in progress. Finish or close one, then start signing in again from the app.")).toBeVisible();
  await expect(page.getByTestId("connect-claim")).toBeVisible();
});
