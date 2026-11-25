import { expect, test, type Page } from "@playwright/test";
// Scenarios first: it loads the fixture's handlers before the registration module's cycle
// through recovery, which a worker loading this file alone would otherwise hit.
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { checkUsername } from "../src/fixture/registration";
import { seriousAxeViolations } from "./axe";
import { payFrom, payFromDashWallet, platformRunFails, setShielded, shieldedRunFails } from "./dashpay";
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

/** Step 1 for `label`, then Continue once the network answered; a contested label's instant
 * offer is declined (No, thanks). */
async function chooseName(page: Page, label: string) {
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  if (checkUsername(label).contested) await page.getByTestId("instant-offer-decline").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 2 of 2 · Confirm");
}

/** From the Join page: Create username, `label`, Confirm & pay, the passphrase. The progress
 * screen shows the new row. */
async function register(page: Page, label = LABEL) {
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await payFromDashWallet(page);
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
  await payFromDashWallet(page);
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

test("IOS-067 / IOS-068: Confirm & pay, the progress through its steps on the page and the Home card, then the welcome", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await payFromDashWallet(page);
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
  await expect(screen(page).locator("h2")).toHaveText(`Welcome to DashPay, @${LABEL}`);
  await expect(page.getByTestId("toolbar-chip")).toContainText(`@${LABEL}`);
  await page.getByTestId("registration-wallet").click();
  await expect(page.getByTestId("hero")).toBeVisible();
  await toJoin(page);
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
  // No proof was recorded before the quit: the engine says chainLock, and no key is held (D21).
  await expect(screen(page)).toHaveAttribute("data-waiting", "chainLock");
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
  await payFromDashWallet(page);
  await chooseName(page, LABEL);
  await page.evaluate(() => window.__dwFixture!.relaunch());
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(card(page)).toHaveAttribute("data-state", "join");
  expect(await assetLocks(page)).toBe(0);
});

test("C-funding F19: Make your username private lists Shielded first and Dash wallet, nothing chosen; Platform behind Advanced; Continue and Back", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await expect(page.getByTestId("join-pay-another")).toHaveCount(0);
  await page.getByTestId("join-create").click();
  const step = page.getByTestId("registration-pay-with");
  await expect(step.getByRole("heading", { level: 2 })).toHaveText("Make your username private");
  await expect(page.getByTestId("privacy-body")).toHaveText("To help keep your payment activity private, we recommend paying for the username from your Shielded balance");
  await expect(step).toHaveAttribute("data-kind", "sources");
  const radio = (s: string) => page.getByTestId(`pay-with-${s}`).getByRole("radio");
  await expect(page.getByTestId("pay-with-sources").or(step.locator(".pay-with-source"))).toHaveCount(2);
  await expect(step.locator(".pay-with-source").first()).toHaveAttribute("data-testid", "pay-with-shielded");
  await expect(page.getByTestId("pay-with-shielded-note")).toHaveText("Keeps your username private");
  await expect(page.getByTestId("pay-with-core-note")).toHaveText("Funds will be traceable to your username");
  await expect(radio("shielded")).not.toBeChecked();
  await expect(radio("core")).not.toBeChecked();
  await expect(page.getByTestId("registration-next")).toBeDisabled();
  await expect(radio("platform")).toHaveCount(0);
  await page.getByTestId("privacy-advanced").click();
  await expect(radio("platform")).toBeEnabled();
  expect(await seriousAxeViolations(page)).toEqual([]);

  await radio("core").check();
  await page.getByTestId("registration-next").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 1 of 2 · Choose a username");
  await chooseName(page, LABEL);
  await page.getByTestId("registration-back").click();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 1 of 2 · Choose a username");
  // Step 1's Back returns to the step, Dash wallet still chosen; Cancel leaves.
  await page.getByTestId("registration-back").click();
  await expect(radio("core")).toBeChecked();
  await page.getByTestId("registration-cancel").click();
  await expect(page.getByTestId("join-create")).toBeVisible();
  // Opened again, nothing is chosen.
  await page.getByTestId("join-create").click();
  await expect(radio("core")).not.toBeChecked();
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "start_registration").length)).toBe(0);
  expect(errors).toEqual([]);
});

test("C-funding F16: while Shielded cannot pay, a tip card and its two buttons replace the sources; Shield your funds first waits while resting or the pool is small", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  const step = page.getByTestId("registration-pay-with");
  const tip = page.getByTestId("privacy-tip");
  const shieldFirst = page.getByTestId("privacy-shield-first");
  const FUNDED = await page.evaluate(() => window.__dwFixture!.world.pending.shielded!.balance);
  const states: { world: Record<string, unknown>; kind: string; title: string; enabled: boolean }[] = [
    { world: { balance: "0", notes: undefined }, kind: "nothing", title: "Privacy tip", enabled: true },
    { world: { balance: "1000000000", notes: undefined }, kind: "partly", title: "Not enough shielded funds", enabled: true },
    { world: { balance: FUNDED, restingUntil: 1_900_000_000 }, kind: "resting", title: "Funds are resting", enabled: false },
    { world: { balance: FUNDED, restingUntil: undefined, poolNotes: 120 }, kind: "pool", title: "Privacy pool too small", enabled: false },
  ];
  for (const s of states) {
    await setShielded(page, s.world);
    await page.getByTestId("join-create").click();
    await expect(step).toHaveAttribute("data-kind", "tip");
    await expect(tip).toHaveAttribute("data-kind", s.kind);
    await expect(tip.locator(".privacy-tip-title")).toHaveText(s.title);
    await expect(page.getByTestId("pay-with-shielded")).toHaveCount(0);
    if (s.enabled) await expect(shieldFirst).toBeEnabled();
    else await expect(shieldFirst).toBeDisabled();
    await expect(page.getByTestId("privacy-without")).toHaveText("Continue without privacy");
    await page.getByTestId("registration-cancel").click();
  }
  // Shield your funds first opens Transfer, Dash wallet → Shielded.
  await setShielded(page, { balance: "0", restingUntil: undefined, poolNotes: undefined });
  await page.getByTestId("join-create").click();
  await shieldFirst.click();
  await expect(page.getByTestId("transfer-sheet")).toBeVisible();
  await expect(page.getByTestId("transfer-from")).toContainText("Dash wallet");
  await expect(page.getByTestId("transfer-to")).toContainText("Shielded");
  expect(errors).toEqual([]);
});

test("C-funding F5, F20: a username paid from the Shielded balance confirms its denomination, fee and change, and registers", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await payFrom(page, "shielded");
  await chooseName(page, LABEL);
  const cost = page.getByTestId("registration-cost");
  await expect(page.getByTestId("registration-fee")).toHaveText("0.03 tDASH");
  await expect(cost).toContainText("0.03 tDASH leaves your Shielded balance.");
  await expect(cost).toContainText("Network fee: about");
  await expect(cost).toContainText("returns to your Shielded balance after the next sync.");
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  const draft = await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft);
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.fromShielded)).toBe(true);
  await page.evaluate((d) => {
    while (window.__dwFixture!.advanceRegistration(d));
  }, draft);
  await expect(page.getByTestId("registration-screen")).toHaveAttribute("data-kind", "done");
  expect(errors).toEqual([]);
});

test("C-funding F12: a Shielded payment whose funds were spent with no username claims no refund and offers a new registration", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await shieldedRunFails(page, LABEL, "consumed");
  await expect(page.getByTestId("registration-lines")).toContainText("Check your Platform balance before you start a new registration.");
  await expect(page.getByTestId("registration-lines")).not.toContainText("registration.payment_consumed");
  await expect(page.getByTestId("registration-another")).toHaveText("Start a new registration");
  expect(errors).toEqual([]);
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
  await payFromDashWallet(page);
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
  await expect(page.getByTestId("registration-cost")).toHaveText("Paid from your identity's credits: up to 2,000,000,000 credits");
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  await expect(screen(page).locator("h2")).toHaveText("Welcome to DashPay, @alice-78");
  expect(await assetLocks(page)).toBe(1);
});

test("D5: a shortfall against what the wallet can spend holds Confirm & pay", async ({ page }) => {
  await open(page, "empty");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    if (f.world.pending.shielded) f.world.pending.shielded = { ...f.world.pending.shielded, balance: "0", notes: undefined };
    f.emit({ kind: "eventsOverflowed" } as never);
  });
  await toJoin(page);
  // C-funding F16: neither the Dash wallet nor the Shielded balance can pay.
  await expect(page.getByTestId("join-shortfall")).toHaveText(
    "You need at least 0.031 tDASH to create a username. Your Dash wallet has 0.00 tDASH. Your Shielded balance has 0.00 tDASH.",
  );
  await page.getByTestId("join-create").click();
  await payFromDashWallet(page);
  await chooseName(page, LABEL);
  await expect(page.getByTestId("registration-shortfall")).toHaveText("You need at least 0.0300226 tDASH to create this username.");
  await expect(page.getByTestId("registration-pay")).toBeDisabled();
});

/** The row of `draft` as a Platform-funded registration whose payment is held: it may still go
 * through, and nothing runs it until Continue (#685). */
async function holdPlatformPayment(page: Page, draft: string) {
  await page.evaluate((d) => {
    const f = window.__dwFixture!;
    const dp = f.world.dashpay;
    Object.assign(dp.registrations.find((r) => r.draft === d)!, { phase: "keysPrepared", waiting: "paymentHeld", running: false, fundsCommitted: true, fromPlatform: true });
    dp.runs = dp.runs.filter((x) => x !== d);
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "registration", draft: d } });
  }, draft);
}

/** The held line names both actions, so it shows only beside both (MG). */
const HELD_LINE =
  "Dash Wallet can't tell yet whether the payment from your Platform balance went through. It may still go through. Choosing Continue tries the same payment again, so it can't pay twice. To free your Platform balance instead, choose Move to Dash wallet. Only one of the two can happen: if the move lands first, the registration isn't paid.";

const calls = (page: Page, name: string) => page.evaluate((n) => window.__dwFixture!.calls.filter((c) => c === n).length, name);

test("#685: a held Platform payment offers Continue and Move to Dash wallet on the progress screen and the Home card", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const draft = await register(page);
  await holdPlatformPayment(page, draft);
  const transfer = page.getByTestId("transfer-body");

  // The progress screen: the line names both, and both are offered.
  await expect(screen(page)).toHaveAttribute("data-kind", "held");
  await expect(page.getByTestId("registration-lines")).toContainText(HELD_LINE);
  await expect(page.getByTestId("registration-continue")).toHaveText("Continue");
  await expect(page.getByTestId("registration-moveToCore")).toHaveText("Move to Dash wallet");
  await page.getByTestId("registration-moveToCore").click();
  await expect(page.getByTestId("page-home")).toBeVisible();
  await expect(transfer).toHaveAttribute("data-route", "platformToCore");
  await page.keyboard.press("Escape");
  await expect(transfer).toHaveCount(0);

  // The Home card: the same two.
  await expect(card(page)).toHaveAttribute("data-state", "creatingHeld");
  await expect(card(page)).toContainText(HELD_LINE);
  await expect(card(page).getByTestId("dashpay-card-action")).toHaveText("Continue");
  await expect(card(page).getByTestId("dashpay-card-secondary")).toHaveText("Move to Dash wallet");
  await card(page).getByTestId("dashpay-card-secondary").click();
  await expect(transfer).toHaveAttribute("data-route", "platformToCore");
  await page.keyboard.press("Escape");
  await expect(transfer).toHaveCount(0);

  // Continue: into the Join page, then the row's own grant (no quote), then the same draft runs.
  const quotes = await calls(page, "registration_quote");
  await card(page).getByTestId("dashpay-card-action").click();
  await expect(screen(page)).toHaveAttribute("data-kind", "held");
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  expect(await calls(page, "registration_resume_grant")).toBe(1);
  expect(await calls(page, "resume_registration")).toBe(1);
  expect(await calls(page, "registration_quote")).toBe(quotes);
  expect(await calls(page, "start_registration")).toBe(1);
  expect(errors).toEqual([]);
});

test("C-funding F4, F7, F22: paid from the Platform balance, a start waits on an unknown answer, parks when held, and Continue finishes it", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await page.evaluate(() => (window.__dwFixture!.world.dashpay.platformCreates = ["unknown", "held"]));
  const credits = () => page.evaluate(() => window.__dwFixture!.world.pending.platform!.addresses.map((a) => a.credits));
  const before = await credits();
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await payFrom(page, "platform");
  await chooseName(page, LABEL);
  // Confirm: what leaves the Platform balance, the identity's funding and the fee.
  await expect(page.getByTestId("registration-cost")).toContainText(
    "About 0.020415 tDASH leaves your Platform balance: 0.02 tDASH for your identity and a Platform fee of about 0.000415 tDASH. Your identity starts with a balance of 0.02 tDASH.",
  );
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  const draft = await draftOf(page);
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.fromPlatform)).toBe(true);
  expect(await assetLocks(page)).toBe(0);

  // F7: no answer yet; the screen waits and offers nothing.
  await advance(page, draft);
  await advance(page, draft);
  await expect(screen(page)).toHaveAttribute("data-waiting", "paymentUnknown");
  await expect(page.getByTestId("registration-continue")).toHaveCount(0);
  // F22: failed by height; Continue and Move to Dash wallet.
  await advance(page, draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "held");
  await expect(page.getByTestId("registration-lines")).toContainText(HELD_LINE);
  await expect(page.getByTestId("registration-moveToCore")).toBeVisible();
  expect(await credits()).toEqual(before);

  // Continue signs the stored plan again under the row's own grant; the run finishes.
  await page.getByTestId("registration-continue").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await complete(page, draft);
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  expect(await calls(page, "start_registration")).toBe(1);
  expect(await calls(page, "registration_resume_grant")).toBe(1);
  expect(await calls(page, "resume_registration")).toBe(1);
  // One debit: the funding and the fee, from input 0.
  const after = await credits();
  expect(BigInt(before[0]) - BigInt(after[0])).toBe(2_041_500_000n);
  expect(after[1]).toBe(before[1]);
  expect(errors).toEqual([]);
});

test("Mainnet: off (flag platform_registration): the privacy step offers no Platform, not even behind Advanced", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-no-identity", "light", "&network=mainnet"));
  await page.getByTestId("toolbar").waitFor();
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await expect(page.getByTestId("registration-pay-with")).toHaveAttribute("data-kind", /^(sources|tip)$/);
  await expect(page.getByTestId("privacy-advanced")).toHaveCount(0);
  await expect(page.getByTestId("pay-with-platform")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("R4 a failed registration whose funding is unknown goes to Repair, which resolves it (Sol r1 P2)", async ({ page }) => {
  await open(page, "dashpay-registration-failed-unknown");
  await expect(card(page)).toHaveAttribute("data-state", "creatingRepair");
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "repair");
  await expect(page.getByTestId("registration-lines")).toContainText("Your funds are safe.");
  await page.getByTestId("registration-repair").click();
  const stuck = page.getByTestId("repair-stuck");
  await expect(stuck).toContainText("alice");
  await stuck.getByRole("button").first().click();
  await page.getByTestId("repair-confirm-ok").click();
  await authorize(page);
  await expect(page.getByTestId("repair-outcome")).toContainText("All coins were sent to yourself");
  await expect(stuck).toBeHidden();
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

  // D22: the status no longer names it (Home offers Join), but the Join page shows it.
  await open(page, "dashpay-registration-stopped");
  await expect(card(page)).toHaveAttribute("data-state", "join");
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "stopped");
  // D27: a new name is offered, privacy first (C-funding F16); leaving shows the stopped row again.
  await page.getByTestId("registration-another").click();
  await expect(page.getByTestId("registration-pay-with")).toBeVisible();
  await page.getByTestId("registration-cancel").click();
  await expect(screen(page)).toHaveAttribute("data-kind", "stopped");
  await page.getByTestId("registration-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
});

test("C-funding F10: a create from the Platform balance that fails in a block says what was kept and offers a new registration", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const credits = () => page.evaluate(() => window.__dwFixture!.world.pending.platform!.addresses.map((a) => a.credits));
  const before = await credits();
  await toJoin(page);
  await platformRunFails(page, LABEL, "paidFailure");
  await expect(screen(page)).toContainText("Registration failed");
  await expect(page.getByTestId("registration-lines")).toHaveText(
    "Dash Platform could not create the identity with this wallet's keys and kept a Platform fee and a penalty from your Platform balance for the attempt. The rest of your Platform balance was not used.",
  );
  await expect(page.getByTestId("registration-repair")).toHaveCount(0);
  await expect(page.getByTestId("registration-continue")).toHaveCount(0);
  await expect(page.getByTestId("registration-another")).toHaveText("Start a new registration");
  // Only the fee left input 0, and the card offers Join again.
  const after = await credits();
  expect(BigInt(before[0]) - BigInt(after[0])).toBe(41_500_000n);
  expect(after[1]).toBe(before[1]);
  expect(await calls(page, "start_registration")).toBe(1);
  await page.getByTestId("registration-another").click();
  await payFromDashWallet(page);
  await expect(page.getByTestId("registration-name")).toBeVisible();
  expect(errors).toEqual([]);
});

test("C-funding F22: a Platform run whose plan changed says so, and Continue goes back to Confirm with a new quote", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const credits = () => page.evaluate(() => window.__dwFixture!.world.pending.platform!.addresses.map((a) => a.credits));
  const before = await credits();
  await toJoin(page);
  await platformRunFails(page, LABEL, "replanRefused");
  await expect(screen(page)).toContainText("Registration failed");
  await expect(page.getByTestId("registration-lines")).toHaveText(
    "No identity was paid for. If you chose Move to Dash wallet for this registration, those credits are in your Dash wallet now, less that move's fee. Review the cost and confirm to try again.",
  );
  await expect(page.getByTestId("registration-lines")).not.toContainText("Nothing was paid");
  await expect(page.getByTestId("registration-lines")).not.toContainText("registration.quote_changed");
  await expect(page.getByTestId("registration-repair")).toHaveCount(0);
  await expect(page.getByTestId("registration-another")).toHaveCount(0);
  expect(await credits()).toEqual(before);
  // The balances change before Continue: no address alone pays now, so the plan takes two inputs.
  await page.evaluate(() => {
    const p = window.__dwFixture!.world.pending.platform!;
    const [a, b] = p.addresses;
    p.addresses = [
      { ...a, credits: "1200000000" },
      { ...b, credits: "1200000000" },
    ];
  });
  const quotes = await calls(page, "registration_quote");
  await expect(page.getByTestId("registration-reconfirm")).toHaveText("Review the cost");
  await page.getByTestId("registration-reconfirm").click();
  // Confirm for the same name from the Platform balance, with the new quote; nothing starts by itself.
  await expect(page.getByTestId("registration-cost")).toContainText(
    "About 0.02042 tDASH leaves your Platform balance: 0.02 tDASH for your identity and a Platform fee of about 0.00042 tDASH.",
  );
  await expect(page.getByTestId("registration-pay")).toBeEnabled();
  expect(await calls(page, "registration_quote")).toBeGreaterThan(quotes);
  expect(await calls(page, "start_registration")).toBe(1);
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.length)).toBe(0);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  // A start with the old quote's numbers would be refused (registration.quote_changed).
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  expect(await calls(page, "start_registration")).toBe(2);
  expect(errors).toEqual([]);
});

test("C-funding F4: a start refused for a changed cost writes nothing, and Pay waits for the new cost", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await payFrom(page, "platform");
  await page.getByTestId("registration-name-field").fill(LABEL);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await expect(page.getByTestId("registration-cost")).toContainText("a Platform fee of about 0.000415 tDASH.");
  // The balances change after the quote: no address alone pays now, so the plan takes two inputs.
  await page.evaluate(() => {
    const p = window.__dwFixture!.world.pending.platform!;
    const [a, b] = p.addresses;
    p.addresses = [
      { ...a, credits: "1200000000" },
      { ...b, credits: "1200000000" },
    ];
  });
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(page.getByTestId("registration-error")).toHaveText("The cost changed. Review it and confirm again. Nothing was paid.");
  // The new cost, read after the refusal, is the one Pay pays.
  await expect(page.getByTestId("registration-cost")).toContainText(
    "About 0.02042 tDASH leaves your Platform balance: 0.02 tDASH for your identity and a Platform fee of about 0.00042 tDASH.",
  );
  await expect(page.getByTestId("registration-pay")).toBeEnabled();
  expect(await calls(page, "start_registration")).toBe(1);
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.length)).toBe(0);
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  expect(await calls(page, "start_registration")).toBe(2);
  expect(errors).toEqual([]);
});

/** Every screen of the wizard, reached from its scenario. */
const SCREENS: { name: string; scenario: string; reach: (page: Page) => Promise<void> }[] = [
  {
    name: "name",
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await payFromDashWallet(page);
      await page.getByTestId("registration-name-field").fill("alice");
      await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
    },
  },
  {
    name: "confirm",
    scenario: "dashpay-no-identity",
    reach: async (page) => {
      await page.getByTestId("join-create").click();
      await payFromDashWallet(page);
      await chooseName(page, "alice");
    },
  },
  { name: "progress", scenario: "dashpay-registering", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "progress")) },
  { name: "paused", scenario: "dashpay-registration-paused", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "paused")) },
  { name: "retry", scenario: "dashpay-registering-paused", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "retry")) },
  { name: "held", scenario: "dashpay-registering-held", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "held")) },
  { name: "repair", scenario: "dashpay-registration-needs-finish", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "repair")) },
  { name: "repair-unknown", scenario: "dashpay-registration-failed-unknown", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "repair")) },
  { name: "stopped", scenario: "dashpay-registration-stopped", reach: async (page) => void (await expect(screen(page)).toHaveAttribute("data-kind", "stopped")) },
  { name: "refunded", scenario: "dashpay-no-identity", reach: (page) => platformRunFails(page, LABEL, "paidFailure") },
  { name: "quote-changed", scenario: "dashpay-no-identity", reach: (page) => platformRunFails(page, LABEL, "replanRefused") },
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

// DP1-09 D4 reversed: the instant username beside a contested one.
test.describe("the instant username beside a contested one", () => {
  async function offer(page: Page) {
    await open(page, "dashpay-no-identity");
    await toJoin(page);
    await page.getByTestId("join-create").click();
    await payFromDashWallet(page);
    await page.getByTestId("registration-name-field").fill("alice");
    await expect(page.getByTestId("registration-next")).toBeEnabled();
    await page.getByTestId("registration-next").click();
    await expect(page.getByTestId("instant-offer")).toBeVisible();
  }
  const startRequests = (page: Page) => page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.map((r) => [r.label, r.temporaryLabel]));

  test("the offer: Create instant username is primary and focused, and Enter takes it", async ({ page }) => {
    await offer(page);
    const create = page.getByTestId("instant-offer-create");
    await expect(page.getByTestId("instant-offer")).toContainText("Create an instant username");
    await expect(page.getByTestId("instant-offer")).toContainText("will be voted on for 1 hour 30 minutes.");
    await expect(create).toBeFocused();
    await expect(create).toHaveClass(/dash-button-filledBlue/);
    await expect(page.getByTestId("instant-offer-decline")).not.toHaveClass(/dash-button-filledBlue/);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("instant-offer")).toHaveCount(0);
    await expect(page.getByTestId("registration-instant")).toBeVisible();
    await expect(page.getByTestId("registration-instant-field")).toHaveValue("alice2");
    await expect(page.getByTestId("registration-instant-explainer")).toContainText("While “alice” is out for a vote");
  });

  test("the form's checks, then one start carrying the instant name, then iOS's success line", async ({ page }) => {
    await offer(page);
    await page.getByTestId("instant-offer-create").click();
    const field = page.getByTestId("registration-instant-field");
    const line = page.getByTestId("registration-instant-check");
    await field.fill("a");
    await expect(line).toHaveAttribute("data-kind", "rules");
    await field.fill("alicia");
    await expect(line).toHaveAttribute("data-kind", "contested");
    await expect(line).toHaveText("This username would also need a vote. Add a digit from 2 to 9, or use 20 characters or more.");
    await expect(page.getByTestId("registration-next")).toBeDisabled();
    await field.fill("alice3");
    await expect(line).toHaveText("Username taken");
    await expect(page.getByTestId("registration-next")).toBeDisabled();
    await field.fill("alice2");
    await expect(line).toHaveText("Username available");
    await page.getByTestId("registration-next").click();
    await expect(page.getByTestId("registration-confirm-instant")).toHaveText("You chose @alice2 as an instant username. It can't be changed once it's registered.");
    await expect(page.getByTestId("registration-fee")).toHaveText("0.2500226 tDASH");
    await page.getByTestId("registration-accept").check();
    await page.getByTestId("registration-pay").click();
    await authorize(page);
    expect(await startRequests(page)).toEqual([["alice", "alice2"]]);
    await complete(page, await draftOf(page));
    await expect(screen(page)).toHaveAttribute("data-kind", "voting");
    await expect(page.getByTestId("registration-lines")).toContainText(
      "In the meantime you are reachable at “alice2”, which is yours to keep. If the vote awards you “alice”, you will be reachable at both usernames.",
    );
    expect(await assetLocks(page)).toBe(1);
  });

  test("No, thanks: Confirm for the contested name alone", async ({ page }) => {
    await offer(page);
    await page.getByTestId("instant-offer-decline").click();
    await expect(page.getByTestId("registration-step")).toHaveText("Step 2 of 2 · Confirm");
    await expect(page.getByTestId("registration-confirm-instant")).toHaveCount(0);
    await page.getByTestId("registration-accept").check();
    await page.getByTestId("registration-pay").click();
    await authorize(page);
    expect(await startRequests(page)).toEqual([["alice", null]]);
  });

  test("refused: the failure line, and Try again opens the instant form with that name", async ({ page }) => {
    await offer(page);
    await page.evaluate(() => (window.__dwFixture!.world.dashpay.temporaryRefused = true));
    await page.getByTestId("instant-offer-create").click();
    await expect(page.getByTestId("registration-instant-check")).toHaveText("Username available");
    await page.getByTestId("registration-next").click();
    await page.getByTestId("registration-accept").check();
    await page.getByTestId("registration-pay").click();
    await authorize(page);
    await complete(page, await draftOf(page));
    await expect(page.getByTestId("registration-lines")).toContainText(
      "The instant username “alice2” could not be registered, so you have no username to use while the vote runs.",
    );
    await page.getByTestId("registration-instantRetry").click();
    await expect(page.getByTestId("registration-name")).toContainText("Create instant username");
    await expect(page.getByTestId("registration-name-field")).toHaveValue("alice2");
  });

  test("a plain name gets no offer", async ({ page }) => {
    await open(page, "dashpay-no-identity");
    await toJoin(page);
    await page.getByTestId("join-create").click();
    await payFromDashWallet(page);
    await chooseName(page, LABEL);
    await expect(page.getByTestId("instant-offer")).toHaveCount(0);
  });
});
