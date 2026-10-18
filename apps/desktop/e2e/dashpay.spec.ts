import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { CARDS } from "./dashpay";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

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
const chip = (page: Page) => page.getByTestId("toolbar-chip");

for (const theme of THEMES) {
  for (const [state, { scenario, extra, title, chip: chipText }] of Object.entries(CARDS)) {
    test(`IOS-065 / AND-024 Home card ${state} ${theme}: title, chip, 0 serious axe issues`, async ({ page }) => {
      const errors = await open(page, scenario, theme, extra);
      await expect(card(page)).toHaveAttribute("data-state", state === "votingNoEnd" ? "voting" : state);
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

test("D2: Home has no card for an identity with a username, or one whose names are not known (S16, S18, D23)", async ({ page }) => {
  await open(page, "dashpay-registered");
  await expect(page.getByTestId("history-header")).toBeVisible();
  await expect(chip(page)).toContainText("@alice");
  await expect(card(page)).toHaveCount(0);
  for (const [scenario, text] of [
    ["dashpay-names-updating", "Updating…"],
    ["dashpay-names-pending", "Updating…"],
    ["dashpay-names-unknown", "Username not loaded"],
  ]) {
    await open(page, scenario);
    await expect(page.getByTestId("history-header")).toBeVisible();
    await expect(chip(page), scenario).toContainText(text);
    await expect(card(page), scenario).toHaveCount(0);
  }
});

/** A page `recordDashPay` records. */
type Recorded = typeof window & { __seen: string[] };

/** Records every card state and chip text the page shows, in order, as `window.__seen`. */
function recordDashPay() {
  const seen: string[] = [];
  (window as Recorded).__seen = seen;
  const note = () => {
    const state = document.querySelector('[data-testid="dashpay-card"]')?.getAttribute("data-state") ?? "none";
    const entry = `${state}|${document.querySelector('[data-testid="toolbar-chip"]')?.textContent ?? ""}`;
    if (seen.at(-1) !== entry) seen.push(entry);
  };
  new MutationObserver(note).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
}

/** The engine's names pass ends: Alice's identity now shows `names`. */
async function namesPassEnds(page: Page, names: string[]) {
  await page.evaluate(
    ({ walletId, names }) => {
      const f = window.__dwFixture!;
      f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, names, mainName: names[0] ?? null, namesCheck: null }));
      f.emit({ kind: "platform", network: "testnet", walletId, change: { domain: "names" } });
    },
    { walletId: FIXTURE_WALLET_ID, names },
  );
}

test("D23 (Sol r1 #1): names that take more than 10 s to load never read as missing; the pass's end settles them", async ({ page }) => {
  test.setTimeout(60_000);
  await page.addInitScript(recordDashPay);
  await open(page, "dashpay-names-pending");
  await expect(chip(page)).toContainText("Updating…");
  // Past the 10 s the UI once waited before asking for a username (D21).
  await page.waitForTimeout(11_000);
  await expect(chip(page)).toContainText("Updating…");
  await expect(card(page)).toHaveCount(0);
  await namesPassEnds(page, ["alice"]);
  await expect(chip(page)).toContainText("@alice");
  const seen = await page.evaluate(() => (window as Recorded).__seen);
  expect(seen.length).toBeGreaterThan(1);
  expect(seen.filter((s) => s.startsWith("noUsername|") || s.includes("No username"))).toEqual([]);
});

test("D23: a names pass that ends with no name asks for one at once", async ({ page }) => {
  await open(page, "dashpay-names-pending");
  await expect(chip(page)).toContainText("Updating…");
  await namesPassEnds(page, []);
  await expect(card(page)).toHaveAttribute("data-state", "noUsername");
  await expect(chip(page)).toContainText("No username");
});

test("D24 (Sol r1 #2): a refetch that fails after a read shows the error, hides the chip and offers no Join", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await expect(card(page)).toHaveAttribute("data-state", "join");
  await expect(chip(page)).toHaveText("Join DashPay");
  const invalidate = () =>
    page.evaluate((walletId) => {
      const f = window.__dwFixture!;
      f.emit({ kind: "platform", network: "testnet", walletId, change: { domain: "startup" } });
    }, FIXTURE_WALLET_ID);
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_status", "none"));
  await invalidate();
  await expect(card(page)).toHaveAttribute("data-state", "error");
  await expect(chip(page)).toHaveCount(0);
  await expect(page.getByTestId("sidebar-joinDashPay")).toHaveCount(0);
  // The Join page too: its card is the error, with no Create username.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await expect(page.getByTestId("join-readiness")).toHaveAttribute("data-kind", "card");
  await expect(page.getByTestId("join-readiness").getByTestId("dashpay-card")).toHaveAttribute("data-state", "error");
  await expect(page.getByTestId("join-create")).toHaveCount(0);
  // Try again, once the read works.
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_status", null));
  await page.getByTestId("join-readiness").getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("join-create")).toBeEnabled();
  await expect(chip(page)).toHaveText("Join DashPay");
  expect(errors).toEqual([]);
});

test("IOS-065 the card's lines: a running step, a paused registration's reason, the vote's end and temporary name", async ({ page }) => {
  await open(page, "dashpay-registering");
  await expect(card(page).getByTestId("dashpay-card-step")).toHaveText("(1/3) Processing payment");
  await expect(card(page).getByTestId("dashpay-card-lines")).toHaveText("Waiting for the network to confirm the payment.");
  // D25 (Sol r1 #3): the live region carries the username and the step as well.
  await expect(card(page).getByRole("status")).toContainText("Creating – @alice");
  await expect(card(page).getByRole("status")).toContainText("(1/3) Processing payment");

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
  for (const scenario of ["dashpay-no-identity", "dashpay-registering-paused", "dashpay-no-username"]) {
    await open(page, scenario);
    await card(page).getByTestId("dashpay-card-action").click();
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

test("IOS-065 J1: the intro and the cost; Create username opens the wizard's first step, Cancel comes back", async ({ page }) => {
  const errors = await openJoin(page, "dashpay-no-identity");
  await expect(page.getByTestId("join-intro").locator("h2")).toHaveText("Create username");
  await expect(page.getByTestId("join-readiness")).toHaveAttribute("data-kind", "join");
  await expect(page.getByTestId("join-cost")).toHaveText(
    "A username costs about 0.031 tDASH. Usernames the network votes on cost more; the exact cost is shown before you pay.",
  );
  await expect(page.getByTestId("join-shortfall")).toHaveCount(0);
  await page.getByTestId("join-create").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 1 of 2 · Choose a username");
  await page.getByTestId("registration-cancel").click();
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

test("IOS-065 J4–J6: a registration in flight shows its screen on the Join page (DP1-09 D12); a joined wallet says so", async ({ page }) => {
  await openJoin(page, "dashpay-registering-paused");
  await expect(page.getByTestId("registration-screen")).toHaveAttribute("data-kind", "retry");
  await expect(page.getByTestId("registration-continue")).toBeVisible();

  await openJoin(page, "dashpay-registered");
  await expect(page.getByTestId("join-joined")).toHaveText("You have joined DashPay as @alice.");
});

test("an invitation link says joining with one is not available yet", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "invitation", linkId: "" }]));
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
