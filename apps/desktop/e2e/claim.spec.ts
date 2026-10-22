import { expect, test, type Page } from "@playwright/test";
import type { InvitationStatus } from "../src/api/generated/InvitationStatus";
import { IDENTITY } from "../src/fixture/dashpay";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { MAIN_PHRASE } from "../src/fixture/vault";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP5-03: the invitation claim screen on the fixture backend. Both entries (welcome: the link
// replayed after onboarding; Join: a link opened in a wallet, from Links you opened or Join's
// paste), every state of the link (valid, used, expired, not valid, a wallet with an identity),
// the inviter never shown as verified and our own claim read as in progress (F7), the request's
// recipient and its opt-out before Claim (n3, F2), the claim's progress and its Done note, and a
// claim resumed after a relaunch. The engine's claim is tested in Rust; the fixture stands in.

const LABEL = "carol-77";

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.locator("[data-testid=onboarding], [data-testid=toolbar]").first().waitFor();
  return errors;
}

const claim = (page: Page) => page.getByTestId("claim");
const screen = (page: Page) => page.getByTestId("registration-screen");
const linkId = (page: Page) => page.evaluate(() => window.__dwFixture!.world.stash[0].info.id);
const lastRow = (page: Page) => page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!);
const setStatus = (page: Page, status: InvitationStatus) =>
  page.evaluate((s) => {
    const w = window.__dwFixture!.world;
    w.invitations = { [w.stash[0].info.id]: s };
  }, status);
const complete = (page: Page, draft: string) =>
  page.evaluate((d) => {
    while (window.__dwFixture!.advanceRegistration(d));
  }, draft);

async function axe(page: Page) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
}

/** The launch's "Links you opened" offer, its invitation opened (the Join entry). */
async function openOffered(page: Page) {
  const sheet = page.getByTestId("saved-links");
  await sheet.getByTestId("saved-link").filter({ hasText: "DashPay invitation" }).getByTestId("saved-link-open").click();
  await expect(sheet).toHaveCount(0);
}

/** `openOffered`, on the claim screen. */
async function openSaved(page: Page) {
  await openOffered(page);
  await expect(claim(page)).toHaveAttribute("data-entry", "join");
}

/** Accept, `label` on step 1, Continue to the claim's step 2. */
async function toConfirm(page: Page, label = LABEL) {
  await page.getByTestId("claim-accept").click();
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await expect(page.getByTestId("registration-confirm")).toHaveAttribute("data-claim", "");
}

test("Join entry: valid → name → recipient → Claim → progress → Done, with the request sent", async ({ page }) => {
  const errors = await open(page, "invitation-valid");
  await openSaved(page);
  await expect(claim(page)).toHaveAttribute("data-state", "invited");
  // F7 (D4): what DPNS proves, never "verified".
  // The proved @username leads; the profile's display name is only beside it (Opus r1 nit).
  await expect(page.getByTestId("claim-inviter-primary")).toHaveText("@bob");
  await expect(page.getByTestId("claim-inviter")).toContainText("Bob Builder");
  await expect(page.getByTestId("claim-inviter-check")).toHaveText(
    "Dash Wallet checked that @bob is a DashPay username. It cannot check who sent you this link.",
  );
  await expect(claim(page)).not.toContainText(/verified/i);
  await expect(claim(page)).toContainText("Paid by the invitation: nothing is paid from this wallet.");

  // D15: a name the network votes on is refused for this invitation.
  await page.getByTestId("claim-accept").click();
  await page.getByTestId("registration-name-field").fill("carol");
  await expect(page.getByTestId("registration-instant-only")).toHaveText(
    "This invitation cannot pay for a username the network votes on. Use a number 2–9, or 20 characters or more.",
  );
  await expect(page.getByTestId("registration-next")).toBeDisabled();
  await page.getByTestId("registration-name-field").fill(LABEL);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();

  // n3 (D6): the recipient and what the request carries, the identity whole, before Claim.
  await expect(page.getByTestId("claim-paid")).toBeVisible();
  await expect(page.getByTestId("claim-request")).toBeChecked();
  await expect(page.getByTestId("claim-recipient-text")).toContainText("A contact request will go to @bob.");
  await expect(page.getByTestId("claim-recipient-text")).toContainText("extended public key");
  await expect(page.getByTestId("claim-recipient")).toContainText(IDENTITY.bob);
  await axe(page);
  await page.getByTestId("registration-pay").click();
  await authorize(page);

  // C1 (D10): the claim's own steps.
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  const progress = page.getByTestId("registration-progress-step");
  await expect(progress).toContainText("(1/3) Checking the invitation");
  const row = await lastRow(page);
  expect(row.invitation).toMatchObject({ inviter: IDENTITY.bob, requestFailure: null });
  await page.evaluate((d) => window.__dwFixture!.advanceRegistration(d), row.draft);
  await expect(progress).toContainText("(2/3) Creating identity");
  await page.evaluate((d) => window.__dwFixture!.advanceRegistration(d), row.draft);
  await expect(progress).toContainText("(3/3) Registering username");
  await complete(page, row.draft);

  // C2: Done, and the request went out.
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(page.getByTestId("registration-lines")).toContainText("A contact request was sent to your inviter.");
  await axe(page);
  await page.getByTestId("registration-done").click();
  await expect(claim(page)).toHaveCount(0);
  await expect(page.getByTestId("join-joined")).toBeVisible();
  expect(errors).toEqual([]);
});

test("F2 (D7): the opt-out asks no one; a request refused for good says so on Done", async ({ page }) => {
  await open(page, "invitation-valid");
  await openSaved(page);
  await toConfirm(page);
  await page.getByTestId("claim-request").uncheck();
  await expect(page.getByTestId("claim-recipient-text")).toHaveCount(0);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  let row = await lastRow(page);
  expect(row.invitation).toMatchObject({ inviter: null });
  await complete(page, row.draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(page.getByTestId("registration-lines")).not.toContainText("contact request");

  await open(page, "invitation-valid");
  await page.evaluate(() => (window.__dwFixture!.world.dashpay.requestRefusal = "contact.ineligible"));
  await openSaved(page);
  await toConfirm(page);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  row = await lastRow(page);
  await complete(page, row.draft);
  await expect(page.getByTestId("registration-lines")).toContainText(
    "The contact request to your inviter could not be sent. You can add them from Contacts.",
  );
});

test("C6: an invitation already used is said so; Dismiss forgets it", async ({ page }) => {
  await open(page, "invitation-claimed");
  await openSaved(page);
  await expect(claim(page)).toHaveAttribute("data-state", "claimed");
  await expect(claim(page)).toContainText("This invitation has already been used");
  await expect(page.getByTestId("claim-accept")).toHaveCount(0);
  await page.getByTestId("claim-dismiss").click();
  await expect(claim(page)).toHaveCount(0);
  expect(await page.evaluate(() => window.__dwFixture!.world.stash.length)).toBe(0);
});

test("F7 (D5): our own claim in flight reads in progress, not already used", async ({ page }) => {
  await open(page, "invitation-claiming");
  // The network says claimed: our claim landed or is landing.
  await setStatus(page, { kind: "claimed" });
  await openOffered(page);
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await expect(claim(page)).toHaveCount(0);
  await expect(screen(page)).toHaveAttribute("data-kind", "paused");
  await expect(page.getByTestId("registration-progress-step")).toContainText("(2/3) Creating identity");
});

test("C1: a claim stopped by a relaunch resumes from Home with no duffs, to Done", async ({ page }) => {
  const errors = await open(page, "invitation-claiming");
  await page.getByTestId("saved-links-later").click();
  await page.evaluate(() => window.__dwFixture!.relaunch());
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await page.getByTestId("saved-links-later").click();
  const card = page.getByTestId("dashpay-card");
  await expect(card).toHaveAttribute("data-state", "creatingPaused");
  await expect(card).toContainText("(2/3) Creating identity");
  await card.getByTestId("dashpay-card-action").click();
  await expect(screen(page)).toHaveAttribute("data-kind", "paused");
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  // D9: the invitation's own grant, nothing in duffs.
  const grant = await page.evaluate(() => window.__dwFixture!.world.grants.at(-1)!.grant.purpose);
  expect(grant).toMatchObject({ kind: "platformOp", maxDuffs: 0 });
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await complete(page, "claim-1");
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(page.getByTestId("registration-lines")).toContainText("A contact request was sent to your inviter.");
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.assetLocks)).toBe(0);
  expect(errors).toEqual([]);
});

test("C3, C7, C8: a wallet with an identity, an expired link, a link that is not valid", async ({ page }) => {
  await open(page, "dashpay-registered");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "invitation", linkId: "" }]));
  await expect(claim(page)).toHaveAttribute("data-state", "hasIdentity");
  await page.getByTestId("claim-notNow").click();
  await expect(claim(page)).toHaveCount(0);
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();

  await open(page, "invitation-valid");
  await setStatus(page, { kind: "expired" });
  await openSaved(page);
  await expect(claim(page)).toHaveAttribute("data-state", "expired");
  await expect(claim(page)).toContainText("This invitation is no longer saved");

  await open(page, "invitation-invalid");
  await openSaved(page);
  await expect(claim(page)).toHaveAttribute("data-state", "invalid");
  await expect(claim(page)).toContainText("This invitation link is not valid");
  await open(page, "invitation-valid");
  await setStatus(page, { kind: "invalid", reason: "assetLockNotFound" });
  await openSaved(page);
  await expect(claim(page)).toContainText("Try again later");
  await expect(page.getByTestId("claim-retry")).toBeVisible();
});

test("Welcome entry (D2): an invitation opened before the wallet opens its claim screen after onboarding", async ({ page }) => {
  const errors = await open(page, "no-wallet-invitation");
  await page.getByTestId("onboarding-restore").click();
  await page.getByTestId("restore-input").fill(MAIN_PHRASE);
  await page.getByTestId("restore-continue").click();
  await page.getByTestId("restore-finish").click();
  await page.getByTestId("onboarding-open").click();
  await expect(claim(page)).toHaveAttribute("data-entry", "welcome");
  await expect(claim(page)).toHaveAttribute("data-state", "invited");
  await expect(claim(page)).toContainText("You have been invited to DashPay");
  await expect(page.getByTestId("saved-links")).toHaveCount(0);
  await page.getByTestId("claim-notNow").click();
  await expect(page.getByTestId("page-home")).toBeVisible();
  // Not now keeps it (D16).
  expect(await page.evaluate(() => window.__dwFixture!.world.stash.length)).toBe(1);
  expect(errors).toEqual([]);
});

test("D14: Join's paste stashes an invitation and opens it; other text is refused", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await page.getByTestId("join-invitation").click();
  const field = page.getByTestId("claim-paste-field");
  await field.fill("https://example.com/not-an-invitation");
  await page.getByTestId("claim-paste-ok").click();
  await expect(page.getByTestId("claim-paste-error")).toHaveText("This is not a DashPay invitation link.");
  await field.fill("dashpay://invite?du=bob&assetlocktx=00ff&pk=cMadeUpKey&islock=00");
  await page.getByTestId("claim-paste-ok").click();
  await expect(page.getByTestId("claim-paste")).toHaveCount(0);
  await expect(claim(page)).toHaveAttribute("data-state", "invited");
  expect(await linkId(page)).toMatch(/^[0-9a-f]{32}$/);
});

for (const theme of THEMES) {
  test(`DP5-03 claim screen ${theme}: 0 serious axe issues`, async ({ page }) => {
    const errors = await open(page, "invitation-valid", theme);
    await openSaved(page);
    await expect(page.getByTestId("claim-inviter")).toBeVisible();
    await axe(page);
    await toConfirm(page);
    await axe(page);
    await open(page, "invitation-claimed", theme);
    await openSaved(page);
    await expect(claim(page)).toHaveAttribute("data-state", "claimed");
    await axe(page);
    expect(errors).toEqual([]);
  });
}
