import { expect, test, type Locator, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { IDENTITY } from "../src/fixture/dashpay";
import { fixtureUrl, fontsReady, trackPageErrors } from "./helpers";
import { armRecorder, expectSaid, heardBy, liveRegion, liveState } from "./liveRegion";

// H-05 O5a: each DashPay task done with the keyboard alone (Tab, Shift+Tab, Enter, Space, Escape,
// arrows and typing) on the fixture backend, with an axe pass at every new screen and dialog. A
// modal sheet must be named and `aria-modal`, take focus, keep Tab inside, and give focus back to
// its opener on Escape. A gap found is a `defect:` test at the end that marks itself
// `test.fail` just before the assertion the app fails, so a fix shows up as an unexpected pass.

// An axe pass at each screen and a Tab walk both ways in each modal make a long test, which WebKit
// on a loaded machine runs past the default 30 s.
test.describe.configure({ timeout: 90_000 });

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("toolbar").waitFor();
  await fontsReady(page);
  return errors;
}

/** What has focus, for failure messages: its test id, else its tag, role and text. */
const focusName = (page: Page) =>
  page.evaluate(() => {
    const a = document.activeElement;
    if (!a || a === document.body) return "<body>";
    const id = a.getAttribute("data-testid");
    if (id) return id;
    const role = a.getAttribute("role");
    const text = (a.getAttribute("aria-label") ?? a.textContent ?? "").trim().slice(0, 40);
    return `${a.tagName.toLowerCase()}${role ? `[role=${role}]` : ""} "${text}"`;
  });

const holdsFocus = (target: Locator) => target.evaluate((el) => el.contains(document.activeElement));

/** Two frames: the browser has rendered, and moved focus off anything that became disabled. */
const rendered = (page: Page) => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

/** Presses Tab (Shift+Tab with `back`) until `target` has focus: how a keyboard user gets there. */
async function tabTo(page: Page, target: Locator, { back = false, max = 60 } = {}) {
  await target.waitFor();
  const path: string[] = [];
  for (let i = 0; i <= max; i++) {
    if (await target.evaluate((el) => el === document.activeElement)) return;
    await page.keyboard.press(back ? "Shift+Tab" : "Tab");
    path.push(await focusName(page));
  }
  throw new Error(`${back ? "Shift+Tab" : "Tab"} never reached the target; focus went: ${path.join(" → ")}`);
}

/** The sidebar's `route` entry, reached with Tab and opened with Enter. */
async function goTo(page: Page, route: string) {
  await tabTo(page, page.getByTestId(`sidebar-${route}`));
  await page.keyboard.press("Enter");
}

/** A contact row (`testId`) by the name it shows. */
const namedRow = (page: Page, testId: string, name: string) => page.getByTestId(testId).filter({ has: page.locator(".contact-row-name", { hasText: name }) });

async function expectAxeClean(page: Page) {
  expect(await seriousAxeViolations(page)).toEqual([]);
}

/** Tab, then Shift+Tab, at least once more than `dialog` has stops: focus never leaves it. The
 * count only sets how many presses to make; where focus lands is read from the page. */
async function expectTabTrapped(page: Page, dialog: Locator) {
  const stops = await dialog.evaluate(
    (d) => d.querySelectorAll('button:not([disabled]), [href], input:not([disabled]), select, textarea, summary, [tabindex]:not([tabindex="-1"])').length,
  );
  for (const key of ["Tab", "Shift+Tab"]) {
    const outside: string[] = [];
    for (let i = 1; i <= stops + 2; i++) {
      await page.keyboard.press(key);
      if (!(await holdsFocus(dialog))) outside.push(`${key} ×${i}: ${await focusName(page)}`);
    }
    expect(outside, `${key} left the modal`).toEqual([]);
  }
}

/** A newly opened modal sheet: named, `aria-modal`, focus inside, Tab kept in, axe clean. */
async function checkModal(page: Page, dialog: Locator, name: string) {
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAttribute("role", "dialog");
  await expect(dialog).toHaveAttribute("aria-modal", "true");
  await expect(dialog).toHaveAccessibleName(name);
  await expect.poll(() => holdsFocus(dialog)).toBe(true);
  await expectAxeClean(page);
  await expectTabTrapped(page, dialog);
}

/** Escape closes `dialog` and focus is back on `opener`. */
async function escapeReturnsTo(page: Page, dialog: Locator, opener: Locator) {
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
}

/** A Platform write's passphrase prompt, opened from `opener` (focused): the field takes focus;
 * checked as a modal; Escape backs out to `opener` and Enter opens it again; then the passphrase. */
async function authorizeFrom(page: Page, opener: Locator) {
  const sheet = page.getByTestId("grant-sheet");
  const field = sheet.locator("input[type=password]");
  await expect(field).toBeFocused();
  await checkModal(page, sheet, "Authorize");
  await escapeReturnsTo(page, sheet, opener);
  await page.keyboard.press("Enter");
  await expect(field).toBeFocused();
  await page.keyboard.type("demo");
  await page.keyboard.press("Enter");
  await expect(sheet).toHaveCount(0);
}

/** Join DashPay ▸ Create username ▸ `label` ▸ Continue: Step 2 with Confirm & pay focused. */
async function toConfirmAndPay(page: Page, label: string) {
  await goTo(page, "joinDashPay");
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await expectAxeClean(page);

  const votingInfo = page.getByTestId("join-voting-info");
  const voting = page.getByTestId("voting-info");
  await tabTo(page, votingInfo);
  await page.keyboard.press("Enter");
  await checkModal(page, voting, "What is username voting?");
  await escapeReturnsTo(page, voting, votingInfo);

  await tabTo(page, page.getByTestId("join-create"), { back: true });
  await page.keyboard.press("Enter");
  await dashWalletByKeyboard(page);
  // Step 1: the field has focus; availability is a live status as typed.
  const field = page.getByTestId("registration-name-field");
  await expect(field).toBeFocused();
  await expect(page.getByTestId("registration-step")).toHaveText("Step 1 of 2 · Choose a username");
  await expectAxeClean(page);
  const availability = await liveRegion(page, '[data-testid="registration-availability"]');
  await page.keyboard.type(label);
  await expectSaid(availability, "polite", "Username available");
  await expect(field).toBeFocused();
  await page.keyboard.press("Enter");

  await expect(page.getByTestId("registration-step")).toHaveText("Step 2 of 2 · Confirm");
  await expect(page.getByTestId("registration-fee")).toBeVisible();
  await expectAxeClean(page);
  const pay = page.getByTestId("registration-pay");
  await tabTo(page, pay);
  return pay;
}

test("keyboard only: register a username", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const pay = await toConfirmAndPay(page, "alice-77");
  await page.keyboard.press("Enter");
  await authorizeFrom(page, pay);

  // The progress screen: its steps are a polite live region.
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await expect(screen.getByTestId("registration-progress-step")).toHaveText("(1/3) Processing payment");
  const progress = await liveRegion(page, '[data-testid="registration-screen"] .registration-live', { empty: false });
  await expectAxeClean(page);

  // The network moves one step: the same region says it.
  const draft = await page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft);
  await page.evaluate((d) => window.__dwFixture!.advanceRegistration(d), draft);
  await expect(screen.getByTestId("registration-progress-step")).toHaveText("(2/3) Creating identity");
  await expectSaid(progress, "polite", /\(2\/3\) Creating identity/);

  // The network finishes the run; the welcome keeps focus at its title, and its status says it.
  const announced = await liveRegion(page, '[data-testid="registration-announced"]', { empty: false });
  await page.evaluate((d) => {
    while (window.__dwFixture!.advanceRegistration(d));
  }, draft);
  await expect(screen).toHaveAttribute("data-kind", "done");
  const title = screen.getByRole("heading", { level: 2 });
  await expect(title).toHaveText("Welcome to DashPay, @alice-77");
  await expect(title).toBeFocused();
  await expectSaid(announced, "polite", /Welcome to DashPay, @alice-77\./);
  await expect(page.getByTestId("toolbar-chip")).toContainText("@alice-77");
  await expectAxeClean(page);
  expect(errors).toEqual([]);
});

/** Contacts ▸ Add contact ▸ "mal" ▸ Mallory: Send request focused. */
async function toSendRequest(page: Page) {
  await goTo(page, "contacts");
  await expect(page.getByTestId("page-contacts")).toBeVisible();
  await expectAxeClean(page);

  const addButton = page.getByTestId("contacts-add");
  await tabTo(page, addButton);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("add-contact")).toBeVisible();
  await expect(page.getByRole("searchbox", { name: "Search username" })).toBeFocused();
  await expectAxeClean(page);

  await page.keyboard.type("mal");
  await tabTo(page, namedRow(page, "add-contact-hit", "Mallory"));
  await page.keyboard.press("Enter");
  // The candidate's card takes focus at its name.
  const candidate = page.getByTestId("add-contact-name");
  await expect(candidate).toBeFocused();
  await expect(candidate).toHaveText("Mallory");
  await expectAxeClean(page);

  const send = page.getByTestId("add-contact-send");
  await tabTo(page, send);
  return { addButton, send };
}

test("keyboard only: send a contact request", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { addButton, send } = await toSendRequest(page);
  await page.keyboard.press("Enter");
  await authorizeFrom(page, send);

  const outcome = page.locator(".toast-host");
  await expect(outcome).toHaveText("Contact request sent to Mallory.");
  await expect(outcome).toHaveAttribute("role", "status");
  await expect(page.getByTestId("add-contact-line")).toHaveText("Contact request sent.");
  await expect(page.getByTestId("add-contact-show")).toBeFocused();
  await expectAxeClean(page);

  // Close gives focus back to Add contact; Pending lists the request.
  await tabTo(page, page.getByTestId("add-contact-close"), { back: true });
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("add-contact")).toHaveCount(0);
  await expect(addButton).toBeFocused();
  await expect(page.getByTestId("contacts-group-pending").locator(".contact-row-name")).toContainText(["Mallory"]);
  expect(errors).toEqual([]);
});

test("keyboard only: Show Request in Add contact moves focus to the sender's detail (#407)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  await goTo(page, "contacts");
  await expect(page.getByTestId("page-contacts")).toBeVisible();
  await tabTo(page, page.getByTestId("contacts-add"));
  await page.keyboard.press("Enter");
  await expect(page.getByRole("searchbox", { name: "Search username" })).toBeFocused();
  await page.keyboard.type("grace");
  await tabTo(page, namedRow(page, "add-contact-hit", "Grace Hopper"));
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("add-contact-line")).toHaveText("They sent you a contact request.");
  const show = page.getByTestId("add-contact-show");
  await expect(show).toHaveText("Show Request");
  await tabTo(page, show);
  await page.keyboard.press("Enter");

  // Add contact closes with the button that had focus; the sender's detail takes it at its name.
  await expect(page.getByTestId("add-contact")).toHaveCount(0);
  const heading = page.getByTestId("contact-detail").getByRole("heading", { level: 2 });
  await expect(heading).toHaveText("Grace Hopper");
  await expect.poll(async () => ((await heading.evaluate((el) => el === document.activeElement)) ? "the detail heading" : await focusName(page))).toBe("the detail heading");
  await expectAxeClean(page);
  expect(errors).toEqual([]);
});

test("Show Request before the contact list has loaded: the sender's detail takes focus once it shows (#407)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  // The toolbar's own reads of the list (the bell's requests) are done first, so the next read
  // held is the Contacts page's.
  const listReads = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "dashpay_contacts").length);
  await expect(page.getByTestId("toolbar-bell-badge")).toBeVisible();
  let reads = -1;
  await expect.poll(async () => {
    const was = reads;
    reads = await listReads();
    return reads === was;
  }, { intervals: [500] }).toBe(true);
  // The list's next read answers only when released: the link's card offers Show Request (from its
  // own eligibility read) while the list, and so the sender's detail, is not there yet.
  await page.evaluate(() => {
    const w = window as unknown as { __release?: () => void };
    w.__release = window.__dwFixture!.hold("dashpay_contacts");
  });
  await page.evaluate((identity) => window.__dwFixture!.deliverLinks([{ kind: "addContact", identity, username: "grace" }]), IDENTITY.grace);
  const show = page.getByTestId("add-contact-show");
  await expect(show).toHaveText("Show Request");
  await tabTo(page, show);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("add-contact")).toHaveCount(0);
  await rendered(page);
  await expect(page.getByTestId("contact-detail")).toHaveCount(0);

  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  const heading = page.getByTestId("contact-detail").getByRole("heading", { level: 2 });
  await expect(heading).toHaveText("Grace Hopper");
  await expect.poll(async () => ((await heading.evaluate((el) => el === document.activeElement)) ? "the detail heading" : await focusName(page))).toBe("the detail heading");
  expect(errors).toEqual([]);
});

/** Opens the bell's popover from the keyboard; its heading takes focus. */
async function openBell(page: Page, key = "Enter") {
  const bell = page.getByTestId("toolbar-bell");
  await expect(bell).toHaveAttribute("aria-haspopup", "dialog");
  await tabTo(page, bell);
  await page.keyboard.press(key);
  await expect(bell).toHaveAttribute("aria-expanded", "true");
  const popover = page.getByTestId("bell-popover");
  await expect(popover).toHaveAccessibleName("Notifications");
  await expect(popover.getByRole("heading", { name: "Notifications" })).toBeFocused();
  return { bell, popover };
}

/** The bell opened with Escape checked, then with Space: Accept heidi focused. */
async function toBellAccept(page: Page) {
  const { bell, popover } = await openBell(page);
  await expectAxeClean(page);
  await escapeReturnsTo(page, popover, bell);
  await expect(bell).toHaveAttribute("aria-expanded", "false");

  await openBell(page, "Space");
  const heidi = popover.getByTestId("bell-request").filter({ hasText: "heidi" });
  const accept = heidi.getByTestId("bell-accept");
  await expect(accept).toHaveAccessibleName("Accept heidi");
  await tabTo(page, accept);
  return { bell, popover, heidi, accept };
}

test("keyboard only: accept a contact request from the bell", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { bell, popover, heidi, accept } = await toBellAccept(page);
  await page.keyboard.press("Enter");
  await authorizeFrom(page, accept);

  // Shown inline while the popover is open (that a reader hears it is the "bell" test below);
  // focus goes to the request now in heidi's place.
  const outcome = page.locator(".toast-host");
  await expect(outcome).toHaveText("You and heidi are now contacts.");
  await expect(outcome).toHaveAttribute("role", "status");
  await expect(heidi).toHaveCount(0);
  await expect(popover.getByTestId("bell-request").filter({ hasText: "Grace Hopper" }).locator(".bell-row-open")).toBeFocused();
  await expectAxeClean(page);

  await escapeReturnsTo(page, popover, bell);
  await goTo(page, "contacts");
  await expect(page.getByTestId("page-contacts")).toBeVisible();
  await expect(page.getByTestId("contacts-group-contacts").locator(".contact-row-name")).toContainText(["heidi"]);
  await expectAxeClean(page);
  expect(errors).toEqual([]);
});

/** Contacts ▸ arrow keys to heidi's request ▸ Enter (the selection announced): Accept focused. */
async function toPaneAccept(page: Page) {
  await goTo(page, "contacts");
  await expect(page.getByTestId("page-contacts")).toBeVisible();

  const heidi = namedRow(page, "contact-row", "heidi");
  await tabTo(page, heidi);
  await page.keyboard.press("ArrowDown");
  await expect(namedRow(page, "contact-row", "Grace Hopper")).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(heidi).toBeFocused();
  const announcer = await liveRegion(page, '[data-testid="contacts-announcer"]');
  await page.keyboard.press("Enter");
  await expect(heidi).toHaveAttribute("aria-current", "true");
  await expectSaid(announcer, "polite", "heidi, Request received");
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Request received");
  await expectAxeClean(page);

  const accept = page.getByTestId("contact-accept");
  await tabTo(page, accept);
  return accept;
}

test("keyboard only: accept a contact request in the contacts list", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const accept = await toPaneAccept(page);
  await page.keyboard.press("Enter");
  await authorizeFrom(page, accept);

  // The pane shows what happened (that a reader hears it is the "contact pane" test below) and
  // takes focus at what it offers now.
  const outcome = page.locator(".toast-host");
  await expect(outcome).toHaveText("You and heidi are now contacts.");
  await expect(outcome).toHaveAttribute("role", "status");
  await expect(page.getByTestId("contact-pane-actions")).toBeFocused();
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Contact");
  await expect(page.getByTestId("contacts-group-contacts").locator(".contact-row-name")).toContainText(["heidi"]);
  await expectAxeClean(page);
  expect(errors).toEqual([]);
});

/** Pay ▸ To a contact: Grace Hopper, 0.01, with focus on the form's Continue. */
async function toPayReview(page: Page) {
  const recipient = page.getByTestId("pay-address");
  await tabTo(page, recipient);
  await page.keyboard.type("Grace");
  await expect(page.getByRole("option", { name: /Grace Hopper/ })).toBeVisible();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Grace Hopper");
  await tabTo(page, page.getByTestId("pay-amount"));
  await page.keyboard.type("0.01");
  await page.getByTestId("pay-fee-line").waitFor();
  const review = page.getByTestId("pay-continue");
  await expect(review).toBeEnabled();
  await tabTo(page, review);
  return review;
}

test("keyboard only: pay a contact", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  await goTo(page, "pay");
  await expectAxeClean(page);

  const review = await toPayReview(page);
  await expectAxeClean(page);
  await page.keyboard.press("Enter");

  // Confirm: the passphrase field takes focus; Escape goes back to the form's Continue.
  const confirm = page.getByTestId("pay-confirm");
  const passphrase = page.getByTestId("pay-confirm-passphrase");
  await expect(passphrase).toBeFocused();
  await checkModal(page, confirm, "Confirm payment");
  await escapeReturnsTo(page, confirm, review);
  await page.keyboard.press("Enter");
  await expect(passphrase).toBeFocused();
  await page.keyboard.type("demo");
  await page.keyboard.press("Enter");

  // The result: a modal named for what happened, focus on its Close; Escape closes it and focus
  // goes back to the form, ready for another payment.
  const result = page.getByTestId("pay-result");
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expect(result.getByTestId("pay-result-close")).toBeFocused();
  await checkModal(page, result, "Payment sent");
  await escapeReturnsTo(page, result, page.getByTestId("pay-amount"));
  expect(errors).toEqual([]);
});

test("keyboard only: edit my profile", async ({ page }) => {
  const errors = await open(page, "dashpay-avatar");
  await tabTo(page, page.getByTestId("toolbar-chip"));
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("profile-name")).toHaveText("Alice Liddell");
  await expectAxeClean(page);

  const edit = page.getByTestId("profile-edit");
  await tabTo(page, edit);
  await page.keyboard.press("Enter");
  const sheet = page.getByTestId("edit-profile-sheet");
  await checkModal(page, sheet, "Edit Profile");
  await escapeReturnsTo(page, sheet, edit);

  await page.keyboard.press("Enter");
  const name = page.getByTestId("edit-profile-name");
  await tabTo(page, name);
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.type("Alice L.");
  // Escape with an edit asks first; Escape there keeps editing, focus back in the field.
  await page.keyboard.press("Escape");
  const discard = page.getByTestId("edit-profile-discard");
  await checkModal(page, discard, "Discard your changes?");
  await page.keyboard.press("Escape");
  await expect(discard).toHaveCount(0);
  await expect(name).toBeFocused();
  await expect(name).toHaveValue("Alice L.");

  const save = page.getByTestId("edit-profile-save");
  await tabTo(page, save);
  // The toast host is the live region that says the save.
  const toasts = await liveRegion(page, ".toast-host");
  await page.keyboard.press("Enter");
  await authorizeFrom(page, save);
  await expectSaid(toasts, "polite", "Your profile was saved.");
  // The sheet closes on the saved profile, and focus is back on Edit Profile.
  await expect(sheet).toHaveCount(0);
  await expect(page.getByTestId("profile-name")).toHaveText("Alice L.");
  await expect(edit).toBeFocused();
  await expectAxeClean(page);
  expect(errors).toEqual([]);
});

test("#286: after Confirm & pay's passphrase, focus moves to the registration progress screen", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const pay = await toConfirmAndPay(page, "alice-77");
  await page.keyboard.press("Enter");
  await authorizeFrom(page, pay);
  // Confirm & pay is disabled as the run starts and goes with Step 2, so the grant sheet has
  // nowhere to give focus back: the progress screen takes it at its title, not <body>.
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await rendered(page);
  expect(errors).toEqual([]);
  await expect.poll(async () => ((await holdsFocus(screen)) ? "on the progress screen" : await focusName(page))).toBe("on the progress screen");
  const title = screen.getByRole("heading", { level: 2 });
  await expect(title).toBeFocused();
  // The welcome then keeps focus at the title, which gets no new focus event, so it is announced.
  await page.evaluate(() => {
    const d = window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft;
    while (window.__dwFixture!.advanceRegistration(d));
  });
  await expect(screen).toHaveAttribute("data-kind", "done");
  await expect(title).toBeFocused();
  await expect(page.getByTestId("registration-announced")).toHaveText("Welcome to DashPay, @alice-77. Your username is ready. Friends can now find you and pay you by name.");
});

// Each passphrase-gated action of a registration's screen, pressed with Enter, its call held after
// the passphrase as a slow network would: the button is disabled while it runs and the grant sheet
// closes onto it, so the screen's title keeps the focus, not <body>.
const SCREEN_ACTIONS = [
  { action: "continue", label: "Continue", scenario: "dashpay-registration-paused", kind: "paused", call: "resume_registration" },
  { action: "finish", label: "Finish", scenario: "dashpay-registration-needs-finish", kind: "repair", call: "finish_asset_locks" },
] as const;

for (const a of SCREEN_ACTIONS)
  test(`keyboard only: while the registration's ${a.label} runs after its passphrase, focus stays on the screen's title (#316)`, async ({ page }) => {
    const errors = await open(page, a.scenario);
    await goTo(page, "joinDashPay");
    const screen = page.getByTestId("registration-screen");
    await expect(screen).toHaveAttribute("data-kind", a.kind);
    const button = page.getByTestId(`registration-${a.action}`);
    await tabTo(page, button);
    const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), a.call);
    await page.keyboard.press("Enter");
    await authorizeFrom(page, button);
    await expect.poll(() => page.evaluate((c) => window.__dwFixture!.calls.includes(c), a.call)).toBe(true);
    await expect(screen.locator(".registration-actions button").first()).toBeDisabled();
    await rendered(page);
    const title = screen.getByRole("heading", { level: 2 });
    await expect.poll(async () => ((await title.evaluate((el) => el === document.activeElement)) ? "the screen's title" : await focusName(page))).toBe("the screen's title");
    await release.evaluate((r) => r());
    await expect(screen).toHaveAttribute("data-kind", "progress");
    await expect(title).toBeFocused();
    expect(errors).toEqual([]);
  });

test("keyboard only: while the registration's Continue runs with no passphrase to ask, focus goes to the screen's title (#316)", async ({ page }) => {
  const errors = await open(page, "dashpay-registration-paused");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.lock = { ...f.world.lock, state: "unencrypted", encrypted: false };
  });
  await goTo(page, "joinDashPay");
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "paused");
  const button = page.getByTestId("registration-continue");
  await tabTo(page, button);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("resume_registration"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("resume_registration"))).toBe(true);
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  // Continue is gone with the press (the row shows its progress at once), and focus with it.
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await expect(button).toHaveCount(0);
  await rendered(page);
  const title = screen.getByRole("heading", { level: 2 });
  await expect.poll(async () => ((await title.evaluate((el) => el === document.activeElement)) ? "the screen's title" : await focusName(page))).toBe("the screen's title");
  await release.evaluate((r) => r());
  // The call's end re-renders the screen with its actions enabled; focus stays on the title.
  await expect(screen.getByTestId("registration-home")).toBeEnabled();
  await rendered(page);
  await expect(title).toBeFocused();
  expect(errors).toEqual([]);
});

test("keyboard only: Escape from the passphrase of a slow Continue gives focus back to Continue (#316)", async ({ page }) => {
  const errors = await open(page, "dashpay-registration-paused");
  await goTo(page, "joinDashPay");
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "paused");
  const button = page.getByTestId("registration-continue");
  await tabTo(page, button);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("registration_quote"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("registration_quote"))).toBe(true);
  await expect(button).toBeDisabled();
  await rendered(page);
  await release.evaluate((r) => r());
  const sheet = page.getByTestId("grant-sheet");
  await expect(sheet.locator("input[type=password]")).toBeFocused();
  await escapeReturnsTo(page, sheet, button);
  await expect(button).toBeEnabled();
  expect(errors).toEqual([]);
});

test("keyboard only: a Continue that went through moves focus to the title when its row replaces Continue (#316)", async ({ page }) => {
  const errors = await open(page, "dashpay-registration-paused");
  await goTo(page, "joinDashPay");
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "paused");
  const button = page.getByTestId("registration-continue");
  await tabTo(page, button);
  // Every read of the rows answers late, as a slow network would: the call ends first and Continue
  // is enabled again before the progress screen replaces it.
  await page.evaluate(() => {
    const ipc = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, ...rest: unknown[]) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = ipc.invoke.bind(ipc);
    const gate = new Promise<void>((r) => ((window as unknown as { openRows: () => void }).openRows = r));
    ipc.invoke = (cmd, ...rest) => (cmd === "registrations" ? gate.then(() => invoke(cmd, ...rest)) : invoke(cmd, ...rest));
  });
  await page.keyboard.press("Enter");
  await authorizeFrom(page, button);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("resume_registration"))).toBe(true);
  // The call ended before the sheet's close returned focus, so the sheet gave it back to Continue.
  await expect(button).toBeEnabled();
  await rendered(page);
  await expect(button).toBeFocused();
  const title = screen.getByRole("heading", { level: 2 });
  await page.evaluate(() => (window as unknown as { openRows: () => void }).openRows());
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await rendered(page);
  await expect.poll(async () => ((await title.evaluate((el) => el === document.activeElement)) ? "the screen's title" : await focusName(page))).toBe("the screen's title");
  // Focus the user then puts on <body> stays there as the run moves on.
  await screen.getByTestId("registration-lines").locator("p").first().click();
  await expect.poll(() => focusName(page)).toBe("<body>");
  const phase = await screen.getAttribute("data-phase");
  await page.evaluate(() => window.__dwFixture!.advanceRegistration(window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft));
  await expect(screen).not.toHaveAttribute("data-phase", phase ?? "");
  await rendered(page);
  await rendered(page);
  expect(await focusName(page)).toBe("<body>");
  expect(errors).toEqual([]);
});

test("a registration step landing while nothing runs leaves focus where the user put it (#316)", async ({ page }) => {
  const errors = await open(page, "dashpay-registering");
  await goTo(page, "joinDashPay");
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await page.getByRole("button", { name: "Lock wallet" }).focus();
  // A click on the screen's text: focus leaves Lock wallet for <body>, as the user chose.
  await screen.getByTestId("registration-lines").locator("p").first().click();
  await expect.poll(() => focusName(page)).toBe("<body>");
  const phase = await screen.getAttribute("data-phase");
  await page.evaluate(() => window.__dwFixture!.advanceRegistration(window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft));
  await expect(screen).not.toHaveAttribute("data-phase", phase ?? "");
  await rendered(page);
  await rendered(page);
  expect(await focusName(page)).toBe("<body>");
  expect(errors).toEqual([]);
});

test("keyboard only: while the wedge's OK runs after its passphrase, focus stays in the wedge sheet (#316)", async ({ page }) => {
  const errors = await open(page, "tools-stuck-funding");
  // The stuck funding is no longer the status's registration: Join DashPay offers a new one, which
  // the engine refuses behind it, so the wedge asks.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.dashpay.status = { kind: "noIdentity", reason: null };
    const draft = f.world.dashpay.registrations[0].draft;
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "registration", draft } });
  });
  const pay = await toConfirmAndPay(page, "alice-77");
  await page.keyboard.press("Enter");
  await authorizeFrom(page, pay);
  const wedge = page.getByTestId("registration-wedge");
  await expect(wedge).toBeVisible();
  const ok = page.getByTestId("registration-wedge-ok");
  await tabTo(page, ok);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("start_registration_past_unknown_funding"));
  await page.keyboard.press("Enter");
  await authorizeFrom(page, ok);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("start_registration_past_unknown_funding"))).toBe(true);
  await expect(ok).toBeDisabled();
  await rendered(page);
  await expect.poll(async () => ((await wedge.evaluate((el) => el === document.activeElement)) ? "the wedge sheet" : await focusName(page))).toBe("the wedge sheet");
  await release.evaluate((r) => r());
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "progress");
  await expect(screen.getByRole("heading", { level: 2 })).toBeFocused();
  expect(errors).toEqual([]);
});

/** "Make your username private" by keyboard: the Dash wallet row checked with Space, Continue with
 * Enter; nothing is chosen until then. */
async function dashWalletByKeyboard(page: Page) {
  const core = page.getByTestId("pay-with-core").getByRole("radio");
  await expect(page.getByTestId("registration-next")).toBeDisabled();
  // One Tab stop for the group, its first row (Shielded when it can pay); the arrows choose.
  const radios = page.getByTestId("registration-pay-with").getByRole("radio");
  await tabTo(page, radios.first());
  await page.keyboard.press((await radios.count()) > 1 ? "ArrowDown" : "Space");
  await expect(core).toBeChecked();
  await expect(core).toBeFocused();
  await page.keyboard.press("Enter");
}

/** Registers "alice-77" with the keyboard from `dashpay-no-identity`: the progress screen, its
 * Home button focused, and the welcome's status region recorded. */
async function toProgressHome(page: Page) {
  await goTo(page, "joinDashPay");
  await tabTo(page, page.getByTestId("join-create"));
  await page.keyboard.press("Enter");
  await dashWalletByKeyboard(page);
  await expect(page.getByTestId("registration-name-field")).toBeFocused();
  await page.keyboard.type("alice-77");
  await expect(page.getByTestId("registration-availability")).toHaveText("Username available");
  await page.keyboard.press("Enter");
  await tabTo(page, page.getByTestId("registration-pay"));
  await page.keyboard.press("Enter");
  const grant = page.getByTestId("grant-sheet");
  await expect(grant.locator("input[type=password]")).toBeFocused();
  await page.keyboard.type("demo");
  await page.keyboard.press("Enter");
  await expect(grant).toHaveCount(0);
  const screen = page.getByTestId("registration-screen");
  await expect(screen).toHaveAttribute("data-kind", "progress");
  const home = page.getByTestId("registration-home");
  await tabTo(page, home);
  const announced = await liveRegion(page, '[data-testid="registration-announced"]');
  return { screen, home, announced, title: screen.getByRole("heading", { level: 2 }) };
}

const finishRegistration = (page: Page) =>
  page.evaluate(() => {
    const d = window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft;
    while (window.__dwFixture!.advanceRegistration(d));
  });

const WELCOME = "Welcome to DashPay, @alice-77. Your username is ready. Friends can now find you and pay you by name.";

test("a registration that finishes behind a sheet is welcomed once the sheet closes: focus at the title (#317)", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const { screen, home, announced, title } = await toProgressHome(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "about" }));
  const about = page.getByTestId("about");
  await expect.poll(() => holdsFocus(about)).toBe(true);
  await finishRegistration(page);
  await expect(screen).toHaveAttribute("data-kind", "done");
  await expect(home).toHaveCount(0);
  await rendered(page);
  expect(await heardBy(announced)).toEqual([]);
  // Its opener, Home, went with the progress screen, so the sheet cannot give focus back.
  await page.keyboard.press("Escape");
  await expect(about).toHaveCount(0);
  await rendered(page);
  await expect.poll(() => focusName(page)).toBe('h2 "Welcome to DashPay, @alice-77"');
  await expect(title).toBeFocused();
  // The focused title says it: the status does not say it again.
  await rendered(page);
  expect(await heardBy(announced)).toEqual([]);
  expect(errors).toEqual([]);
});

test("a registration that finishes behind a sheet is said by the status once the sheet closes onto other focus (#317)", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const { screen, announced, title } = await toProgressHome(page);
  const other = page.getByTestId("toolbar-discreet");
  await tabTo(page, other, { back: true });
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "about" }));
  const about = page.getByTestId("about");
  await expect.poll(() => holdsFocus(about)).toBe(true);
  await finishRegistration(page);
  await expect(screen).toHaveAttribute("data-kind", "done");
  await rendered(page);
  expect(await heardBy(announced)).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(about).toHaveCount(0);
  await expect(other).toBeFocused();
  await expectSaid(announced, "polite", WELCOME);
  await rendered(page);
  expect(await heardBy(announced)).toEqual([`polite: ${WELCOME}`]);
  await expect(title).not.toBeFocused();
  expect(errors).toEqual([]);
});

test("a sheet reopened in the frame the welcome is said still lets it be heard, once (#317)", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const { screen, announced } = await toProgressHome(page);
  await tabTo(page, page.getByTestId("toolbar-discreet"), { back: true });
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "about" }));
  const about = page.getByTestId("about");
  await expect.poll(() => holdsFocus(about)).toBe(true);
  await finishRegistration(page);
  await expect(screen).toHaveAttribute("data-kind", "done");
  await rendered(page);
  // Escape, then About again in the frame that announces the welcome (after its callback).
  await page.evaluate(async () => {
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await Promise.resolve();
    await Promise.resolve();
    await new Promise<void>((r) =>
      requestAnimationFrame(() => {
        window.__dwFixture!.shellCommand({ kind: "about" });
        r();
      }),
    );
  });
  await expect.poll(() => holdsFocus(about)).toBe(true);
  await rendered(page);
  await page.keyboard.press("Escape");
  await expect(about).toHaveCount(0);
  await rendered(page);
  await rendered(page);
  expect(await heardBy(announced)).toEqual([`polite: ${WELCOME}`]);
  expect(errors).toEqual([]);
});

test("with no sheet open the welcome is announced once (#317)", async ({ page }) => {
  const errors = await open(page, "dashpay-no-identity");
  const { screen, announced, title } = await toProgressHome(page);
  await title.focus();
  await expect(title).toBeFocused();
  await finishRegistration(page);
  await expect(screen).toHaveAttribute("data-kind", "done");
  await expectSaid(announced, "polite", WELCOME);
  await rendered(page);
  await rendered(page);
  expect(await heardBy(announced)).toEqual([`polite: ${WELCOME}`]);
  await expect(title).toBeFocused();
  expect(errors).toEqual([]);
});

/** The bell popover now, read in one go: "closed" (absent or not rendered), "focus kept" (it
 * holds focus), else where focus went while it stays open. */
const bellState = (page: Page) =>
  page.evaluate(() => {
    const p = document.querySelector<HTMLElement>('[data-testid="bell-popover"]');
    if (!p || !p.checkVisibility({ visibilityProperty: true, opacityProperty: true, checkVisibilityCSS: true, checkOpacity: true })) return "closed";
    if (p.contains(document.activeElement)) return "focus kept";
    const a = document.activeElement;
    return `focus on ${a === document.body ? "<body>" : (a?.getAttribute("data-testid") ?? a?.tagName)} with the popover open`;
  });

test("keyboard only: Tab past the bell popover's last control closes it and leaves focus on Hide balance (#287)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { bell, popover } = await openBell(page);
  await tabTo(page, popover.getByTestId("bell-see-all"));
  await page.keyboard.press("Tab");
  await rendered(page);
  await expect.poll(() => bellState(page)).toBe("closed");
  await expect(bell).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByTestId("toolbar-discreet")).toBeFocused();
  expect(errors).toEqual([]);
});

test("keyboard only: Shift+Tab from the bell popover's heading to the bell keeps it open, and Tab goes back in (#287)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { bell, popover } = await openBell(page);
  await page.keyboard.press("Shift+Tab");
  await rendered(page);
  await expect(bell).toBeFocused();
  await expect(popover).toBeVisible();
  await expect(bell).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("Tab");
  await rendered(page);
  await expect.poll(() => bellState(page)).toBe("focus kept");
  expect(errors).toEqual([]);
});

test("keyboard only: Tab through the bell popover's request rows and their actions keeps it open (#287)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { popover } = await openBell(page);
  await expect(popover.getByTestId("bell-request")).not.toHaveCount(0);
  expect(await popover.getByTestId("bell-request").count()).toBeGreaterThan(1);
  const seeAll = popover.getByTestId("bell-see-all");
  const path: string[] = [];
  for (let i = 0; i < 40 && !(await seeAll.evaluate((el) => el === document.activeElement)); i++) {
    await page.keyboard.press("Tab");
    await rendered(page);
    path.push(await focusName(page));
    expect(await bellState(page), `after ${path.join(" → ")}`).toBe("focus kept");
  }
  await expect(seeAll).toBeFocused();
  expect(path.filter((p) => p === "bell-accept").length, path.join(" → ")).toBeGreaterThan(1);
  for (let i = 1; i < path.length; i++) {
    await page.keyboard.press("Shift+Tab");
    await rendered(page);
    expect(await bellState(page)).toBe("focus kept");
  }
  expect(await focusName(page)).toBe(path[0]);
  expect(errors).toEqual([]);
});

test("keyboard only: focus in Accept's passphrase sheet keeps the bell popover open (#287)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { popover } = await openBell(page);
  const accept = popover.getByTestId("bell-request").filter({ hasText: "heidi" }).getByTestId("bell-accept");
  await tabTo(page, accept);
  await page.keyboard.press("Enter");
  const sheet = page.getByTestId("grant-sheet");
  await expect(sheet.locator("input[type=password]")).toBeFocused();
  await expectTabTrapped(page, sheet);
  await expect(popover).toBeVisible();
  await escapeReturnsTo(page, sheet, accept);
  await expect.poll(() => bellState(page)).toBe("focus kept");
  expect(errors).toEqual([]);
});

test("keyboard only: Tab out of the bell popover while its Ignore runs closes it once the Ignore ends (#287)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { bell, popover } = await openBell(page);
  const ignore = popover.getByTestId("bell-request").filter({ hasText: "heidi" }).getByTestId("bell-ignore");
  await tabTo(page, ignore);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("dashpay_ignore_contact"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("dashpay_ignore_contact"))).toBe(true);
  await tabTo(page, popover.getByTestId("bell-see-all"));
  await page.keyboard.press("Tab");
  const discreet = page.getByTestId("toolbar-discreet");
  await expect(discreet).toBeFocused();
  await release.evaluate((r) => r());
  await expect.poll(() => bellState(page)).toBe("closed");
  await expect(bell).toHaveAttribute("aria-expanded", "false");
  await expect(discreet).toBeFocused();
  expect(errors).toEqual([]);
});

test("keyboard only: Escape from inside the bell popover closes it and gives focus back to the bell", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { bell, popover } = await openBell(page);
  await tabTo(page, popover.getByTestId("bell-accept").first());
  await escapeReturnsTo(page, popover, bell);
  await expect(bell).toHaveAttribute("aria-expanded", "false");
  expect(errors).toEqual([]);
});

test("keyboard only: while a contact payment is sent, Confirm payment keeps focus in the dialog, then the result's Close takes it (#288)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  await goTo(page, "pay");
  await toPayReview(page);
  await page.keyboard.press("Enter");
  const passphrase = page.getByTestId("pay-confirm-passphrase");
  await expect(passphrase).toBeFocused();
  await page.keyboard.type("demo");
  // The network holds the broadcast, as a slow one would. Enter disables the passphrase field
  // while the payment is authorized, prepared and broadcast: the dialog itself keeps the focus.
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("draft_broadcast"))).toBe(true);
  const confirm = page.getByTestId("pay-confirm");
  await expect(confirm).toBeVisible();
  await expect(passphrase).toBeDisabled();
  await rendered(page);
  await expect.poll(async () => ((await holdsFocus(confirm)) ? "in Confirm payment" : await focusName(page))).toBe("in Confirm payment");
  // The result replaces it and focuses its Close button.
  await release.evaluate((r) => r());
  await expect(page.getByTestId("pay-result")).toBeVisible();
  await expect(page.getByTestId("pay-result-close")).toBeFocused();
  expect(errors).toEqual([]);
});

// A contact action's result goes in `Outcome` (src/screens/tools/common.tsx). A status region that
// appears already holding its text is not reliably announced (#291), so the empty status region is
// there before the action and that same element is given the result. `reach` gets to the action's
// button; Enter and the passphrase run it.
const OUTCOME_SITES: { where: string; region: string; said: string; reach: (page: Page) => Promise<Locator> }[] = [
  {
    where: "Add contact ▸ Send request (shared toast)",
    region: ".toast-host",
    said: "Contact request sent to Mallory.",
    reach: async (page) => (await toSendRequest(page)).send,
  },
  {
    where: "bell popover ▸ Accept (shared toast)",
    region: ".toast-host",
    said: "You and heidi are now contacts.",
    reach: async (page) => (await toBellAccept(page)).accept,
  },
  {
    where: "contact pane ▸ Accept (shared toast)",
    region: ".toast-host",
    said: "You and heidi are now contacts.",
    reach: toPaneAccept,
  },
  {
    where: "Notifications page ▸ Accept (shared toast)",
    region: ".toast-host",
    said: "You and heidi are now contacts.",
    reach: async (page) => {
      const { popover } = await openBell(page);
      await tabTo(page, popover.getByTestId("bell-see-all"));
      await page.keyboard.press("Enter");
      const list = page.getByTestId("notifications-requests");
      await expect(list).toBeVisible();
      const accept = list.getByTestId("bell-request").filter({ hasText: "heidi" }).getByTestId("bell-accept");
      await tabTo(page, accept);
      return accept;
    },
  },
];

for (const site of OUTCOME_SITES) {
  test(`${site.where}: the result is said by the status region present before the action (#291)`, async ({ page }) => {
    const errors = await open(page, "dashpay-registered");
    const trigger = await site.reach(page);
    const region = await page.evaluateHandle((s) => document.querySelector(s), site.region);
    const before = await region.evaluate(liveState);
    await armRecorder(page, region);
    await page.keyboard.press("Enter");
    await authorizeFrom(page, trigger);
    await expect(page.locator(site.region)).toHaveText(site.said);
    expect(errors).toEqual([]);
    await expect
      .poll(async () => ({ before, said: (await heardBy(region)).includes(`polite: ${site.said}`) }))
      .toEqual({ before: "polite: ", said: true });
  });
}
