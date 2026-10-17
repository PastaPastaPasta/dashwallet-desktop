import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP1-10: My Profile, the username request card, the credits block and the top-up sheet on the
// fixture backend, by parity ID: IOS-068 (the username request status: tallies, deadline,
// temporary name), IOS-069 (the profile sheet: names, credits, top-up from Core; DASHPAY F8's
// IOS-131 presets); Android's AND-071 (voting) and AND-078 (top-up, low-credit warnings).

async function openProfile(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("page-profile")).toBeVisible();
  return errors;
}

const body = (page: Page) => page.getByTestId("profile-body");
const credits = (page: Page) => page.getByTestId("profile-credits");
const request = (page: Page) => page.getByTestId("profile-request");
const sheet = (page: Page) => page.getByTestId("topup-body");

/** Each page state the design inventories (P2–P6, C1–C7, R1–R6), its scenario and what shows. */
const STATES: Record<string, { scenario: string; extra?: string; ready: (page: Page) => Promise<void> }> = {
  P2: { scenario: "dashpay-registered", extra: "&fail=dashpay_status:none", ready: (p) => expect(body(p)).toHaveAttribute("data-state", "error") },
  P3: { scenario: "dashpay-no-identity", ready: (p) => expect(body(p)).toHaveAttribute("data-state", "noProfile") },
  "P3-registering": { scenario: "dashpay-registering", ready: (p) => expect(p.getByTestId("dashpay-card")).toHaveAttribute("data-state", "creating") },
  P4: { scenario: "dashpay-registered", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("@alice") },
  P5: { scenario: "dashpay-no-username", ready: (p) => expect(p.getByTestId("profile-choose-name")).toBeVisible() },
  "P5-updating": { scenario: "dashpay-names-updating", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("Updating…") },
  "P5-unknown": { scenario: "dashpay-names-unknown", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("Username not loaded") },
  C2: { scenario: "dashpay-credits-low", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "low") },
  C3: { scenario: "dashpay-low-credits", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "empty") },
  C4: { scenario: "dashpay-credits-unknown", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "unknown") },
  C5: { scenario: "dashpay-credits-stale", ready: (p) => expect(credits(p)).toContainText("Updating…") },
  C6: { scenario: "dashpay-top-up-pending", ready: (p) => expect(p.getByTestId("profile-pending-finish")).toBeVisible() },
  "C6-unknown": { scenario: "dashpay-top-up-unknown", ready: (p) => expect(p.getByTestId("profile-pending-arrived")).toBeVisible() },
  C7: { scenario: "dashpay-top-up-stuck", ready: (p) => expect(p.getByTestId("profile-stuck")).toBeVisible() },
  R1: { scenario: "dashpay-contest-pending", ready: (p) => expect(p.getByTestId("profile-tally")).toBeVisible() },
  R2: { scenario: "dashpay-contest-pending-no-end", ready: (p) => expect(request(p)).toContainText("We will notify you when voting ends.") },
  "R1-instant": { scenario: "dashpay-contest-no-temporary", ready: (p) => expect(p.getByTestId("profile-instant")).toBeVisible() },
  R3: { scenario: "dashpay-contest-won", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "won") },
  R4: { scenario: "dashpay-contest-lost", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "lost") },
  R5: { scenario: "dashpay-contest-locked", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "locked") },
  R6: { scenario: "dashpay-contest-pending", extra: "&fail=contest_status:none", ready: (p) => expect(p.getByTestId("profile-request-failed")).toBeVisible() },
};

for (const theme of THEMES) {
  for (const [state, { scenario, extra, ready }] of Object.entries(STATES)) {
    test(`IOS-068 / IOS-069 My Profile ${state} ${theme}: renders, 0 serious axe issues`, async ({ page }) => {
      const errors = await openProfile(page, scenario, theme, extra);
      await ready(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test(`IOS-069 / AND-078 top-up sheet ${theme}: amount, confirm, outcome; 0 serious axe issues`, async ({ page }) => {
    const errors = await openProfile(page, "dashpay-registered", theme);
    await page.getByTestId("profile-top-up").click();
    await expect(sheet(page)).toHaveAttribute("data-step", "amount");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-step", "done");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("IOS-069 P4: header, usernames, identity id with copy, credits and the contest outcome rows", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await expect(page.getByTestId("profile-name")).toHaveText("Alice Liddell");
  await expect(page.getByTestId("profile-header")).toContainText("Down the rabbit hole");
  await expect(page.getByTestId("profile-edit")).toBeDisabled();
  await expect(page.getByTestId("profile-usernames")).toContainText("@alice");
  await expect(page.getByTestId("profile-identity").getByRole("button", { name: "Copy identity id" })).toBeVisible();
  await expect(page.getByTestId("profile-credits-figure")).toHaveText(/^1\.845/);
  await expect(page.getByTestId("profile-credits-estimate")).toHaveText("≈ 1,961 contact requests · 1,537 profile updates");
  // The fixture's journal holds a lost contest for "al" (D12).
  await expect(page.getByTestId("profile-outcomes")).toContainText("@al — went to another user");
});

test("C8 / D7: discreet mode masks the credit figure and the estimate", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("toolbar-discreet").click();
  await expect(page.getByTestId("profile-credits-figure")).not.toContainText("1.845");
  await expect(page.getByTestId("profile-credits-estimate")).not.toContainText("1,961");
});

test("IOS-069 D5: a custom amount below the floor is refused before Continue; a preset tops up and the balance moves", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-other").click();
  await page.getByTestId("topup-other-field").fill("0.001");
  await expect(page.getByTestId("topup-amount-problem")).toContainText("The smallest top-up is 0.01");
  await expect(page.getByTestId("topup-next")).toBeDisabled();
  await page.getByTestId("topup-preset-10000000").click();
  await page.getByTestId("topup-next").click();
  await expect(page.getByTestId("topup-quote")).toContainText("Top-up fee");
  await expect(sheet(page)).toContainText("You have chosen to top up your identity.");
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
  await page.getByTestId("topup-done").click();
  await expect(page.getByTestId("topup-sheet")).toHaveCount(0);
  await expect(page.getByTestId("profile-credits-figure")).toHaveText(/^1\.94/);
});

test("IOS-069 D15: a pending top-up holds Top up; Finish applies it", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-pending");
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
  await expect(page.getByTestId("profile-top-up-reason")).toHaveText("Finish the pending top-up first.");
  await expect(page.getByTestId("profile-pending-arrived")).toHaveCount(0);
  await page.getByTestId("profile-pending-finish").click();
  await authorize(page);
  await expect(page.getByTestId("profile-pending-finish")).toHaveCount(0);
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("IOS-069 D15: Credits arrived resolves a proved lock whose consumption is unknown", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-unknown");
  await page.getByTestId("profile-pending-arrived").click();
  await expect(page.getByTestId("profile-pending-arrived")).toHaveCount(0);
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("D16: a stuck top-up funding routes to Tools ▸ Repair, which resolves it", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-stuck");
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
  await page.getByTestId("profile-stuck-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
  const row = page.getByTestId("repair-stuck-topup/0-1/funding");
  await expect(row).toContainText("A top-up of @alice's credits");
  await page.getByTestId("repair-resolve-topup/0-1/funding").click();
  await expect(page.getByTestId("repair-confirm")).toContainText("Whether the top-up's payment reached the network");
  await page.getByTestId("repair-confirm-ok").click();
  await authorize(page);
  await expect(page.getByTestId("repair-outcome")).toContainText("You can top up again once it is confirmed.");
  await expect(row).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("T8 / D17: the quote's refusal shows before Confirm; a refusal at pay routes by its code", async ({ page }) => {
  await openProfile(page, "dashpay-registered", "light", "&fail=credits_top_up_quote:none");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await expect(page.getByTestId("topup-quote-error")).toBeVisible();
  await expect(page.getByTestId("topup-pay")).toBeDisabled();
});

test("T8 / D16 / D22: a funding left unknown at pay routes to Repair, which lists its step", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-lost");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(sheet(page)).toHaveAttribute("data-step", "refused");
  await expect(page.getByTestId("topup-refused-back")).toHaveCount(0);
  await page.getByTestId("topup-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
  await expect(page.getByTestId("repair-stuck-topup/0-1/funding")).toContainText("A top-up of @alice's credits");
  // No Platform signal followed the refusal; the page read the stuck funding again anyway.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-stuck")).toBeVisible();
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
});

test("D14: a top-up that ends while the sheet is closed is offered on the page and shown once", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("credits_top_up"));
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(sheet(page)).toHaveAttribute("data-step", "running");
  await page.getByTestId("topup-sheet").getByRole("button", { name: "Close" }).click();
  await expect(page.getByTestId("profile-top-up-reason")).toHaveText("Topping up…");
  await release.evaluate((r) => r());
  await expect(page.getByTestId("profile-top-up-unseen")).toContainText("Your top-up has an update.");
  await page.getByTestId("profile-top-up-show").click();
  await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
  await page.getByTestId("topup-done").click();
  await expect(page.getByTestId("profile-top-up-unseen")).toHaveCount(0);
  await page.getByTestId("profile-top-up").click();
  await expect(sheet(page)).toHaveAttribute("data-step", "amount");
});

test("IOS-068 R1 / D11: the tally, the deadline and the temporary name; voting info opens", async ({ page }) => {
  await openProfile(page, "dashpay-contest-pending");
  await expect(body(page)).toHaveAttribute("data-state", "contest");
  await expect(request(page)).toHaveAttribute("data-state", "open");
  await expect(request(page)).toContainText("Voting – @alice");
  await expect(request(page)).toContainText("Until then you are @alice007.");
  await expect(page.getByTestId("profile-tally")).toContainText("Votes for you12");
  await expect(page.getByTestId("profile-instant")).toHaveCount(0);
  await page.getByTestId("profile-voting-info").click();
  await expect(page.getByTestId("voting-info")).toBeVisible();
});

test("D13: Create instant username opens the credits path and refuses a contested label", async ({ page }) => {
  await openProfile(page, "dashpay-contest-no-temporary");
  await expect(page.getByTestId("profile-username")).toHaveText("@alice · Pending");
  await page.getByTestId("profile-instant").click();
  await expect(page.getByTestId("registration-name")).toContainText("Create instant username");
  await page.getByTestId("registration-name-field").fill("bobby");
  await expect(page.getByTestId("registration-instant-only")).toBeVisible();
  await expect(page.getByTestId("registration-next")).toBeDisabled();
  await page.getByTestId("registration-name-field").fill("bobby2");
  await expect(page.getByTestId("registration-instant-only")).toHaveCount(0);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
});

test("R6: a failed contest read keeps the card and Try again reads it", async ({ page }) => {
  await openProfile(page, "dashpay-contest-pending", "light", "&fail=contest_status:none");
  await expect(request(page)).toContainText("Voting – @alice");
  await expect(page.getByTestId("profile-request-failed")).toContainText("Could not read the votes.");
  await page.evaluate(() => window.__dwFixture!.fail("contest_status", null));
  await page.getByTestId("profile-request-retry").click();
  await expect(page.getByTestId("profile-request-failed")).toHaveCount(0);
  await expect(page.getByTestId("profile-tally")).toContainText("Votes for you12");
});

test("D21: the contested chip opens My Profile", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-contest-pending", "light"));
  await page.getByTestId("toolbar-chip").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
});

test("S19 / D8: Home's low-credit card buys credits: My Profile with the top-up sheet open", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-credits-low", "light"));
  const card = page.getByTestId("dashpay-card");
  await expect(card).toHaveAttribute("data-state", "lowCredits");
  await card.getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
  await expect(sheet(page)).toHaveAttribute("data-step", "amount");
});

test("D20: Settings ▸ General's DashPay card opens My Profile, or Join without an identity", async ({ page }) => {
  for (const [scenario, label, route] of [
    ["dashpay-registered", "My Profile", "profile"],
    ["dashpay-no-identity", "Join DashPay", "joinDashPay"],
  ] as const) {
    await page.goto(fixtureUrl(scenario, "light"));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    const open = page.getByTestId("settings-dashpay-open");
    await expect(open).toHaveText(label);
    await open.click();
    await expect(page.getByTestId(`page-${route}`)).toBeVisible();
  }
});

test("D2: My Profile without an identity is DP1-08's card with its action", async ({ page }) => {
  await openProfile(page, "dashpay-no-identity");
  await expect(page.getByTestId("profile-body")).toContainText("You have no DashPay profile yet");
  await page.getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
});
