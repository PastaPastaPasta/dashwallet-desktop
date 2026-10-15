import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { CARDS } from "./dashpay";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { NAMELESS_HOLD_MS } from "../src/dashpay/status";

// DP1-08: the DashPay status card on Home, the identity chip, the Join DashPay page, the voting
// info and the FAQ on the fixture backend, by parity ID: IOS-065 (the Join DashPay banner states,
// the intro, the voting info), AND-024 (Android's Home banner), AND-069–071 (voting).

async function open(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function openJoin(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = await open(page, scenario, theme, extra);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  return errors;
}

const card = (page: Page) => page.getByTestId("dashpay-card");
/** S17 waits out D21's hold. */
const HELD = { timeout: NAMELESS_HOLD_MS + 10_000 };
const chip = (page: Page) => page.getByTestId("toolbar-chip");

for (const theme of THEMES) {
  for (const [state, { scenario, extra, title, chip: chipText }] of Object.entries(CARDS)) {
    test(`IOS-065 / AND-024 Home card ${state} ${theme}: title, chip, 0 serious axe issues`, async ({ page }) => {
      const errors = await open(page, scenario, theme, extra);
      await expect(card(page)).toHaveAttribute("data-state", state === "votingNoEnd" ? "voting" : state, HELD);
      await expect(card(page).locator("h2")).toHaveText(title);
      // An identity chip leads with its avatar letter.
      if (chipText) await expect(chip(page)).toContainText(chipText);
      else await expect(chip(page)).toHaveCount(0);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

test("D2: Home has no card for an identity with a username, or one whose names are updating (S16, S18)", async ({ page }) => {
  await open(page, "dashpay-registered");
  await expect(page.getByTestId("history-header")).toBeVisible();
  await expect(chip(page)).toContainText("@alice");
  await expect(card(page)).toHaveCount(0);
  await open(page, "dashpay-names-updating");
  await expect(page.getByTestId("history-header")).toBeVisible();
  await expect(chip(page)).toContainText("Updating…");
  await expect(card(page)).toHaveCount(0);
});

test("D21: a nameless identity asks for a username only once it has stayed nameless, and not again after a revisit", async ({ page }) => {
  test.setTimeout(60_000);
  await open(page, "dashpay-no-username");
  await expect(chip(page)).toContainText("No username");
  await page.waitForTimeout(1_000);
  await expect(card(page)).toHaveCount(0);
  await expect(card(page)).toHaveAttribute("data-state", "noUsername", HELD);
  // Away and back: the hold does not start again.
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("sidebar-home").click();
  await expect(card(page)).toHaveAttribute("data-state", "noUsername", { timeout: 2_000 });
});

test("IOS-065 the card's lines: a running step, a paused registration's reason, the vote's end and temporary name", async ({ page }) => {
  await open(page, "dashpay-registering");
  await expect(card(page).getByTestId("dashpay-card-step")).toHaveText("(1/3) Processing payment");
  await expect(card(page).getByRole("status")).toHaveText("Waiting for the network to confirm the payment.");

  await open(page, "dashpay-registering-paused");
  await expect(card(page).getByRole("status")).toContainText("Registration paused");
  await expect(card(page).getByRole("status")).toContainText("Not enough Dash to pay for the username: 0.0300226 tDASH needed, 0.01 tDASH available.");

  await open(page, "dashpay-contest-pending");
  await expect(card(page).getByRole("status")).toContainText("Results on Mar 17, 2030.");
  await expect(card(page).getByRole("status")).toContainText("Until then you are @alice007.");
  await open(page, "dashpay-contest-pending-no-end");
  await expect(card(page).getByRole("status")).toContainText("We will notify you when voting ends.");

  await open(page, "dashpay-waiting-for-sync");
  await expect(card(page).getByTestId("dashpay-card-note")).toHaveText(/^Available after sync finishes · \d+\.\d%$/);
  await expect(card(page).getByTestId("dashpay-card-action")).toBeDisabled();
});

test("the card's buttons: Join, Continue and Choose a username open Join DashPay; Repair opens Tools ▸ Repair", async ({ page }) => {
  test.setTimeout(60_000);
  for (const scenario of ["dashpay-no-identity", "dashpay-registering-paused", "dashpay-no-username"]) {
    await open(page, scenario);
    await card(page).getByTestId("dashpay-card-action").click(HELD);
    await expect(page.getByTestId("page-joinDashPay"), scenario).toBeVisible();
  }
  await open(page, "tools-stuck-funding");
  await expect(card(page).getByTestId("dashpay-card-action")).toHaveText("Open Repair");
  await card(page).getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
});

test("S5 / S12: Unlock opens the unlock sheet while unlocked for mixing only", async ({ page }) => {
  for (const scenario of ["dashpay-identity-unsettled", "dashpay-registering-unlock"]) {
    await open(page, scenario);
    await card(page).getByTestId("dashpay-card-action").click();
    await expect(page.getByTestId("unlock-sheet"), scenario).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("unlock-sheet")).toHaveCount(0);
  }
});

test("S2: Try again reads the status again", async ({ page }) => {
  await open(page, "dashpay-no-identity", "light", "&fail=dashpay_status:none");
  await expect(card(page)).toHaveAttribute("data-state", "error");
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_status", null));
  await card(page).getByTestId("dashpay-card-action").click();
  await expect(card(page)).toHaveAttribute("data-state", "join");
});

test("a Platform{Startup} signal moves the card from looking to Join", async ({ page }) => {
  await open(page, "dashpay-starting");
  await expect(card(page)).toHaveAttribute("data-state", "looking");
  await page.evaluate((walletId) => {
    const f = window.__dwFixture!;
    f.world.dashpay.status = { kind: "noIdentity", reason: null };
    f.emit({ kind: "platform", network: "testnet", walletId, change: { domain: "startup" } });
  }, FIXTURE_WALLET_ID);
  await expect(card(page)).toHaveAttribute("data-state", "join");
  await expect(chip(page)).toHaveText("Join DashPay");
});

test("AND-069–071 / IOS-134 What is username voting? opens the voting info and closes with Escape", async ({ page }) => {
  await open(page, "dashpay-contest-pending");
  await card(page).getByTestId("dashpay-card-action").click();
  const sheet = page.getByTestId("voting-info");
  await expect(sheet).toBeVisible();
  await expect(sheet.locator("h3")).toHaveCount(6);
  await expect(sheet).toContainText("Any username that has a number 2–9 or 20 or more characters will be automatically approved");
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
});

test("IOS-065 J1: the intro and the cost; Create username leads to the wizard placeholder and back", async ({ page }) => {
  const errors = await openJoin(page, "dashpay-no-identity");
  await expect(page.getByTestId("join-intro").locator("h2")).toHaveText("Create username");
  await expect(page.getByTestId("join-readiness")).toHaveAttribute("data-kind", "join");
  await expect(page.getByTestId("join-cost")).toHaveText(
    "A username costs about 0.031 tDASH. Usernames the network votes on cost more; the exact cost is shown before you pay.",
  );
  await expect(page.getByTestId("join-shortfall")).toHaveCount(0);
  await page.getByTestId("join-create").click();
  await expect(page.getByTestId("placeholder-registration")).toHaveText("Choosing and registering a username is not available yet.");
  await page.getByTestId("join-wizard-back").click();
  await expect(page.getByTestId("join-create")).toBeVisible();
  expect(errors).toEqual([]);
});

test("IOS-065 J1 a shortfall, J3 watch-only, J2 held until synced, J7 a failed quote", async ({ page }) => {
  await openJoin(page, "empty");
  await expect(page.getByTestId("join-shortfall")).toHaveText("You need at least 0.031 tDASH to create a username.");

  await openJoin(page, "dashpay-watch-only");
  await expect(page.getByTestId("join-readiness")).toHaveAttribute("data-kind", "watchOnly");
  await expect(page.getByTestId("join-create")).toHaveCount(0);

  await openJoin(page, "dashpay-waiting-for-sync");
  await expect(page.getByTestId("join-create")).toBeDisabled();
  await expect(page.getByTestId("join-waiting")).toHaveText("Available after sync finishes");

  await openJoin(page, "dashpay-no-identity", "light", "&fail=registration_quote:none");
  await expect(page.getByTestId("join-cost")).toHaveText("Usernames the network votes on cost more than others. The exact cost is shown before you pay.");
  await expect(page.getByTestId("join-create")).toBeEnabled();
});

test("IOS-065 J4–J6: a registration's card on the Join page continues into the wizard; a joined wallet says so", async ({ page }) => {
  await openJoin(page, "dashpay-registering-paused");
  await expect(page.getByTestId("join-readiness")).toHaveAttribute("data-kind", "card");
  await page.getByTestId("join-readiness").getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("placeholder-registration")).toBeVisible();

  await openJoin(page, "dashpay-registered");
  await expect(page.getByTestId("join-joined")).toHaveText("You have joined DashPay as @alice.");
});

test("an invitation link says joining with one is not available yet", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "invitation" }]));
  await expect(page.getByTestId("invitation-received")).toBeVisible();
  await expect(page.getByTestId("join-intro")).toBeVisible();
});

for (const theme of THEMES) {
  test(`IOS-129 / IOS-134 Join page, voting info and FAQ ${theme}: 0 serious axe issues`, async ({ page }) => {
    const errors = await openJoin(page, "dashpay-no-identity", theme);
    await expect(page.getByTestId("join-cost")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);

    await page.getByTestId("join-voting-info").click();
    await expect(page.getByTestId("voting-info")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.keyboard.press("Escape");

    await page.getByTestId("join-faq").click();
    const faq = page.getByTestId("dashpay-faq");
    await expect(faq.getByTestId("dashpay-faq-item")).toHaveCount(8);
    // Native disclosures: Enter on a question opens its answer.
    await faq.getByTestId("dashpay-faq-item").first().locator("summary").focus();
    await page.keyboard.press("Enter");
    await expect(faq.getByTestId("dashpay-faq-item").first()).toHaveAttribute("open", "");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(faq).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
