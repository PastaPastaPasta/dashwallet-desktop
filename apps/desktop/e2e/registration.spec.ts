import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP1-09: the username registration wizard on the fixture backend, by parity ID: IOS-066 (choose
// a username: the checklist, availability, the contested explanation), IOS-067 (the cost and
// Confirm & pay), IOS-068 (the progress screen, the result, failure and retry), and the
// acceptance: close the app mid-flow, relaunch, resume, with no second asset lock.

const W = FIXTURE_WALLET_ID;
const LABEL = "alice-77";

async function open(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function toJoin(page: Page) {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
}

const screen = (page: Page) => page.getByTestId("registration-screen");
const card = (page: Page) => page.getByTestId("dashpay-card");
const assetLocks = (page: Page) => page.evaluate(() => window.__dwFixture!.world.dashpay.assetLocks);
const draftOf = (page: Page) => page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft);
/** One step of the run, where the network would answer. */
const advance = (page: Page, draft: string) => page.evaluate((d) => window.__dwFixture!.advanceRegistration(d), draft);
/** Every step left, to the end of the run. */
const complete = (page: Page, draft: string) =>
  page.evaluate((d) => {
    while (window.__dwFixture!.advanceRegistration(d));
  }, draft);

/** Step 1 for `label`, then Continue once the network answered. */
async function chooseName(page: Page, label: string) {
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 2 of 2 · Confirm");
}

/** From the Join page: Create username, `label`, Confirm & pay, the passphrase. The progress
 * screen shows the new row. */
async function register(page: Page, label = LABEL) {
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await chooseName(page, label);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  return draftOf(page);
}

test("IOS-066: the checklist as typed, availability once valid, the contested explanation; taken names stop Continue", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 1 of 2 · Choose a username");
  const field = page.getByTestId("registration-name-field");
  const rules = page.getByTestId("registration-rules").locator("li");
  const next = page.getByTestId("registration-next");

  await field.fill("a-");
  await expect(rules.nth(0)).toHaveAttribute("data-passed", "false");
  await expect(rules.nth(3)).toHaveAttribute("data-passed", "false");
  await expect(field).toHaveAttribute("aria-invalid", "true");
  await expect(next).toBeDisabled();

  await field.fill("b0b");
  await expect(page.getByTestId("registration-availability")).toHaveText("Username taken");
  await expect(next).toBeDisabled();

  await field.fill("alice");
  await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
  await expect(page.getByTestId("registration-contested")).toContainText("The Dash network must vote to approve this username");
  await page.getByTestId("registration-voting-info").click();
  await expect(page.getByTestId("voting-info")).toBeVisible();
  await page.keyboard.press("Escape");

  await field.fill("carol");
  await expect(page.getByTestId("registration-availability")).toContainText("let the network vote");
  await expect(next).toBeEnabled();

  await field.fill(LABEL);
  await expect(page.getByTestId("registration-contested")).toHaveCount(0);
  await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
  await expect(next).toBeEnabled();
  // The field takes the engine's cap and no more.
  await field.fill("x".repeat(30));
  await expect(field).toHaveValue("x".repeat(23));
  expect(errors).toEqual([]);
});

test("IOS-067 / IOS-068: Confirm & pay, the progress through its steps on the page and the Home card, then Hello @name", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await chooseName(page, LABEL);
  await expect(page.getByTestId("registration-fee")).toHaveText("0.0300226 tDASH");
  await expect(page.getByTestId("registration-accept")).toHaveCount(0);
  await page.getByTestId("registration-pay").click();
  await authorize(page);

  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await expect(screen(page)).toHaveAttribute("data-phase", "fundingSent");
  await expect(page.getByTestId("registration-progress-step")).toHaveText("(1/3) Processing payment");
  expect(await assetLocks(page)).toBe(1);
  const draft = await draftOf(page);

  // D11: Home's card and chip say the same.
  await page.getByTestId("registration-home").click();
  await expect(card(page)).toHaveAttribute("data-state", "creating");
  await expect(card(page).getByTestId("dashpay-card-step")).toHaveText("(1/3) Processing payment");
  await advance(page, draft);
  await expect(card(page).getByTestId("dashpay-card-step")).toHaveText("(2/3) Creating identity");

  // The progress survives leaving the page.
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-phase", "proofWaiting");
  await advance(page, draft);
  await expect(page.getByTestId("registration-progress-step")).toHaveText("(3/3) Registering username");
  await complete(page, draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(screen(page).locator("h2")).toHaveText(`Hello @${LABEL},`);
  await expect(page.getByTestId("toolbar-chip")).toContainText(`@${LABEL}`);
  await page.getByTestId("registration-done").click();
  await expect(page.getByTestId("join-joined")).toHaveText(`You have joined DashPay as @${LABEL}.`);
  expect(await assetLocks(page)).toBe(1);
  expect(errors).toEqual([]);
});

test("DP1-09 acceptance: close the app after the asset lock went out, relaunch, Continue: the same lock completes", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const draft = await register(page);
  expect(await assetLocks(page)).toBe(1);

  // Quit and relaunch: the vault opens locked, the run is gone, the row is on disk.
  await page.evaluate(() => window.__dwFixture!.relaunch());
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);

  // Home says the registration is paused; Continue leads to its screen (D12: never resumed alone).
  await expect(card(page)).toHaveAttribute("data-state", "creatingPaused");
  await card(page).getByTestId("dashpay-card-action").click();
  await expect(screen(page)).toHaveAttribute("data-kind", "paused");
  await expect(screen(page)).toHaveAttribute("data-waiting", "authorize");
  expect(await assetLocks(page)).toBe(1);
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await expect(screen(page)).toHaveAttribute("data-waiting", "instantSend");

  await complete(page, draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  // One registration, one asset lock: the resume sent none.
  expect(await assetLocks(page)).toBe(1);
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.length)).toBe(1);
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "start_registration").length)).toBe(0);
  expect(errors).toEqual([]);
});

test("D13: a relaunch on step 1 or 2 starts from the Join card; nothing was paid", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await chooseName(page, LABEL);
  await page.evaluate(() => window.__dwFixture!.relaunch());
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(card(page)).toHaveAttribute("data-state", "join");
  expect(await assetLocks(page)).toBe(0);
});

test("D14: a passphrase change parks the run; the page shows Continue, which resumes it", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  const draft = await register(page);
  await page.evaluate((walletId) => {
    const f = window.__dwFixture!;
    f.world.leases = f.world.leases.map((l) => ({ ...l, state: { kind: "revoked", cause: "passphraseChange" } }));
    for (const lease of f.world.leases.filter((l) => l.walletId === walletId)) f.emit({ kind: "leaseChanged", network: "testnet", lease });
  }, W);
  await expect(screen(page)).toHaveAttribute("data-kind", "paused");
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await advance(page, draft);
  await expect(screen(page)).toHaveAttribute("data-phase", "proofWaiting");
});

test("IOS-068 contested: I accept before paying, then voting has begun", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await chooseName(page, "alice");
  await expect(page.getByTestId("registration-fee")).toHaveText("0.2500226 tDASH");
  await expect(page.getByTestId("registration-cost")).toContainText("The network votes on this username for about two weeks");
  await expect(page.getByTestId("registration-pay")).toBeDisabled();
  await page.getByTestId("registration-accept").check();
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await complete(page, await draftOf(page));
  await expect(screen(page)).toHaveAttribute("data-kind", "voting");
  await expect(screen(page).locator("h2")).toHaveText("Voting for your username has begun");
  await expect(page.getByTestId("toolbar-chip")).toContainText("Pending");
});

test("IOS-068 failures: a retryable stop continues the same registration; a refused name keeps the identity and pays the next one from credits", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  const draft = await register(page);
  await page.evaluate((d) => window.__dwFixture!.failRegistration(d, "platform.unavailable", true), draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "retry");
  await expect(page.getByTestId("registration-lines")).toContainText("Your funds are safe.");
  // D7: a funded registration is never discarded.
  await expect(page.getByTestId("registration-another")).toHaveCount(0);
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");

  await advance(page, draft);
  await advance(page, draft);
  await page.evaluate((d) => window.__dwFixture!.failRegistration(d, "name.taken", false), draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "nameFailed");
  await expect(page.getByTestId("registration-lines")).toHaveText(`@${LABEL} could not be registered. Username was taken by another user.`);
  await page.getByTestId("registration-another").click();
  await chooseName(page, "alice-78");
  await expect(page.getByTestId("registration-cost")).toHaveText("Paid from your identity's credits");
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(screen(page).locator("h2")).toHaveText("Hello @alice-78,");
  expect(await assetLocks(page)).toBe(1);
});

test("D5: a shortfall against what the wallet can spend holds Confirm & pay", async ({ page }) => {
  await open(page, "empty");
  await toJoin(page);
  await expect(page.getByTestId("join-shortfall")).toHaveText("You need at least 0.031 tDASH to create a username.");
  await page.getByTestId("join-create").click();
  await chooseName(page, LABEL);
  await expect(page.getByTestId("registration-shortfall")).toHaveText("You need at least 0.0300226 tDASH to create this username.");
  await expect(page.getByTestId("registration-pay")).toBeDisabled();
});

test("R5 Finish transfers on the page; R8 a stopped funded registration says the funds are safe and opens Repair", async ({ page }) => {
  await open(page, "dashpay-registration-needs-finish");
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "repair");
  await expect(page.getByTestId("registration-lines")).toContainText("Your funds are safe.");
  await page.getByTestId("registration-finish").click();
  await authorize(page);
  await expect(page.getByTestId("registration-finished")).toHaveText("1 resumed, 0 completed, 0 still pending.");
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");

  await open(page, "dashpay-registration-stopped");
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "stopped");
  await page.getByTestId("registration-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
});

/** Every screen of the wizard, reached from its scenario. */
const SCREENS: { name: string; scenario: string; reach: (page: Page) => Promise<void> }[] = [
  {
    name: "name",
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await page.getByTestId("registration-name-field").fill("alice");
      await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
    },
  },
  {
    name: "confirm",
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await chooseName(page, "alice");
    },
  },
  { name: "progress", scenario: "dashpay-registering", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "progress")) },
  { name: "paused", scenario: "dashpay-registration-paused", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "paused")) },
  { name: "retry", scenario: "dashpay-registering-paused", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "retry")) },
  { name: "repair", scenario: "dashpay-registration-needs-finish", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "repair")) },
  { name: "stopped", scenario: "dashpay-registration-stopped", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "stopped")) },
];

for (const theme of THEMES)
  for (const s of SCREENS)
    test(`DP1-09 ${s.name} ${theme}: 0 serious axe issues`, async ({ page }) => {
      const errors = await open(page, s.scenario, theme);
      await toJoin(page);
      await s.reach(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
