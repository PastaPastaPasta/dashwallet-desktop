import { expect, test, type JSHandle, type Locator, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, trackPageErrors } from "./helpers";

// H-05 O5a: each DashPay task done with the keyboard alone (Tab, Shift+Tab, Enter, Space, Escape,
// arrows and typing) on the fixture backend, with an axe pass at every new screen and dialog. A
// modal sheet must be named and `aria-modal`, take focus, keep Tab inside, and give focus back to
// its opener on Escape. The gaps found are the `defect:` tests at the end: each marks itself
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

/** How `el` reads to assistive tech now, in the page: "<politeness>: <text>" (polite or assertive)
 * for an exposed live region; for a live region a reader cannot reach yet, why not and its text;
 * else "absent" or "not a live region". An explicit `aria-live` wins over the role's default (`off`
 * is not live). A region inside aria-busy="true" is not exposed: readers hold its updates. The
 * text is what a reader gets: text inside an aria-hidden, hidden, inert or unrendered element of
 * the region is left out. Self-contained: it runs in the page. */
function liveState(el: Element | null): string {
  if (!el?.isConnected) return "absent";
  const explicit = el.getAttribute("aria-live");
  const politeness = explicit !== null ? explicit.trim() : ({ status: "polite", alert: "assertive", log: "polite" } as Record<string, string>)[el.getAttribute("role") ?? ""];
  if (politeness !== "polite" && politeness !== "assertive") return "not a live region";
  const parts: string[] = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    let shown = true;
    for (let p = node.parentElement; p && p !== el; p = p.parentElement) {
      const style = getComputedStyle(p);
      if (p.getAttribute("aria-hidden") === "true" || p.hasAttribute("hidden") || p.hasAttribute("inert") || style.display === "none" || style.visibility === "hidden") {
        shown = false;
        break;
      }
    }
    if (shown) parts.push((node as Text).data);
  }
  const text = parts.join("").replace(/\s+/g, " ").trim();
  if (el.closest('[aria-hidden="true"], [inert]')) return `hidden from assistive tech: ${text}`;
  if (el.closest('[aria-busy="true"]')) return `busy, so readers hold its updates: ${text}`;
  const modal = document.querySelector('[aria-modal="true"]');
  if (modal && !modal.contains(el)) return `outside the open modal: ${text}`;
  if (!el.checkVisibility({ visibilityProperty: true, checkVisibilityCSS: true })) return `not rendered: ${text}`;
  return `${politeness}: ${text}`;
}

type Heard = Element & { __heard?: string[] };

/** Records what `el` says from now on: at each content change that changes its text for a reader,
 * `liveState` then. A reader announces changed text only, so text that arrived while the region
 * was hidden and was exposed later, and a mutation that leaves the text as it was, say nothing new.
 * A missing `el` records nothing. */
async function armRecorder(page: Page, region: JSHandle<Element | null>) {
  await page.evaluate(`window.__dwLiveState = ${liveState.toString()}`);
  await region.evaluate((el: Heard | null) => {
    if (!el) return;
    const heard: string[] = [];
    el.__heard = heard;
    const read = (window as unknown as { __dwLiveState: typeof liveState }).__dwLiveState;
    const textOf = (state: string) => state.slice(state.indexOf(": ") + 2);
    let last = textOf(read(el));
    new MutationObserver(() => {
      const state = read(el);
      if (textOf(state) === last) return;
      last = textOf(state);
      heard.push(state);
    }).observe(el, { childList: true, subtree: true, characterData: true });
  });
}

const heardBy = (region: JSHandle<Element | null>) => region.evaluate((el: Heard | null) => el?.__heard ?? []);

/** The live region at `selector`, held and recorded before an action, once it is there as a live
 * region (empty, with `empty`), whether or not a reader can reach it yet: what counts is that it
 * is exposed when it speaks. */
async function liveRegion(page: Page, selector: string, { empty = true } = {}) {
  await page.evaluate(`window.__dwLiveState = ${liveState.toString()}`);
  const state = () => page.evaluate((s) => (window as unknown as { __dwLiveState: typeof liveState }).__dwLiveState(document.querySelector(s)), selector);
  const kinds = "polite|assertive|hidden from assistive tech|busy, so readers hold its updates|outside the open modal|not rendered";
  await expect.poll(state, { message: `live region ${selector} before the action` }).toMatch(new RegExp(empty ? `^(${kinds}): $` : `^(${kinds}): `));
  const region = await page.evaluateHandle((s) => document.querySelector(s), selector);
  await armRecorder(page, region);
  return region;
}

/** The element held by `liveRegion` said `text` while exposed, at `politeness`: all its text, or
 * (a RegExp) a match in it. */
async function expectSaid(region: JSHandle<Element | null>, politeness: "polite" | "assertive", text: string | RegExp) {
  const said = typeof text === "string" ? `${politeness}: ${text}` : expect.stringMatching(new RegExp(`^${politeness}: .*${text.source}`));
  await expect.poll(() => heardBy(region)).toContainEqual(said);
}

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

  // The outcome shows (that a reader hears it is the "Add contact" test below), and focus has a
  // place in the card that now says it went.
  const outcome = page.getByTestId("add-contact-outcome");
  await expect(outcome).toHaveText("Contact request sent to Mallory.");
  await expect(outcome).toHaveAttribute("role", "status");
  await expect(page.getByTestId("add-contact-line")).toHaveText("Contact request sent.");
  await expect(page.getByTestId("add-contact-status")).toBeFocused();
  await expectAxeClean(page);

  // Close gives focus back to Add contact; Pending lists the request.
  await tabTo(page, page.getByTestId("add-contact-close"), { back: true });
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("add-contact")).toHaveCount(0);
  await expect(addButton).toBeFocused();
  await expect(page.getByTestId("contacts-group-pending").locator(".contact-row-name")).toContainText(["Mallory"]);
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
  const outcome = page.getByTestId("bell-outcome");
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
  const outcome = page.getByTestId("contact-pane-outcome");
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
  await tabTo(page, namedRow(page, "pay-picker-row", "Grace Hopper"));
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
  const toContact = page.getByTestId("pay-tabs-contact");
  const toAddress = page.getByTestId("pay-tabs-address");
  await expect(toContact).toHaveAttribute("aria-selected", "true");
  await expectAxeClean(page);
  // The tabs follow the arrow keys.
  await tabTo(page, toContact);
  await page.keyboard.press("ArrowRight");
  await expect(toAddress).toBeFocused();
  await expect(toAddress).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pay-address")).toBeVisible();
  await expectAxeClean(page);
  await page.keyboard.press("ArrowLeft");
  await expect(toContact).toBeFocused();
  await expect(toContact).toHaveAttribute("aria-selected", "true");

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

test("defect (#287): Tab past the bell popover's last control moves focus behind it while it stays open", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  const { popover } = await openBell(page);
  await tabTo(page, popover.getByTestId("bell-see-all"));
  await page.keyboard.press("Tab");
  await rendered(page);
  expect(errors).toEqual([]);
  test.fail(
    true,
    "Toolbar bell popover (role=dialog, no aria-modal): Tab from See all goes to the toolbar's Hide balance and Lock wallet, then the sidebar, while the popover stays open over the page; expected focus kept in the popover, or the popover closed when focus leaves it.",
  );
  await expect.poll(() => bellState(page)).toMatch(/^(closed|focus kept)$/);
});

test("defect (#288): while a contact payment is sent, Confirm payment leaves focus on <body>", async ({ page }) => {
  const errors = await open(page, "dashpay-registered");
  await goTo(page, "pay");
  await toPayReview(page);
  await page.keyboard.press("Enter");
  const passphrase = page.getByTestId("pay-confirm-passphrase");
  await expect(passphrase).toBeFocused();
  await page.keyboard.type("demo");
  // The network holds the broadcast, as a slow one would.
  await page.evaluate(() => void window.__dwFixture!.hold("draft_broadcast"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("draft_broadcast"))).toBe(true);
  const confirm = page.getByTestId("pay-confirm");
  await expect(confirm).toBeVisible();
  await expect(passphrase).toBeDisabled();
  await rendered(page);
  expect(errors).toEqual([]);
  test.fail(
    true,
    "Pay ▸ Confirm payment: Enter disables the passphrase field while the payment is authorized, prepared and broadcast, and focus falls to <body> until the result; expected focus kept in the dialog (on the dialog itself or its sending status).",
  );
  await expect.poll(async () => ((await holdsFocus(confirm)) ? "in Confirm payment" : await focusName(page))).toBe("in Confirm payment");
});

// A contact action's result goes in `Outcome` (src/screens/tools/common.tsx). A status region that
// appears already holding its text is not reliably announced (#291), so the empty status region is
// there before the action and that same element is given the result. `reach` gets to the action's
// button; Enter and the passphrase run it.
const OUTCOME_SITES: { where: string; region: string; said: string; reach: (page: Page) => Promise<Locator> }[] = [
  {
    where: "Add contact ▸ Send request (add-contact-outcome)",
    region: '[data-testid="add-contact-outcome"]',
    said: "Contact request sent to Mallory.",
    reach: async (page) => (await toSendRequest(page)).send,
  },
  {
    where: "bell popover ▸ Accept (bell-outcome)",
    region: '[data-testid="bell-outcome"]',
    said: "You and heidi are now contacts.",
    reach: async (page) => (await toBellAccept(page)).accept,
  },
  {
    where: "contact pane ▸ Accept (contact-pane-outcome)",
    region: '[data-testid="contact-pane-outcome"]',
    said: "You and heidi are now contacts.",
    reach: toPaneAccept,
  },
  {
    where: "Notifications page ▸ Accept (notifications-outcome)",
    region: '[data-testid="notifications-outcome"]',
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
