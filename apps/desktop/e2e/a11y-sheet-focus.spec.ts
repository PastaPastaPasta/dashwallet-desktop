import { expect, test, type Locator, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE, type World } from "../src/fixture/scenarios";
import { fixtureUrl, fontsReady, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

// H-05: focus never leaves an open modal sheet, with the keyboard alone, whatever in it has focus:
// the dialog itself when it has nothing to press or a pressed button went disabled while its
// action runs. Tab from the dialog goes to its first control, Shift+Tab to its last, and an action
// that fails gives the focus back to its button. Pay links fill the form; the rest is keys.

test.describe.configure({ timeout: 90_000 });

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select, textarea, summary, [tabindex]:not([tabindex="-1"])';

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function open(page: Page, scenario: string) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId(scenario === "locked" ? "lock-field" : "toolbar").waitFor();
  await fontsReady(page);
  return errors;
}

/** What has focus: "dialog" (the sheet itself), its first or last control, another control in it by
 * test id, or what outside it has focus. */
const focusIn = (dialog: Locator) =>
  dialog.evaluate((d, selector) => {
    const a = document.activeElement;
    if (a === d) return "dialog";
    if (!a || a === document.body) return "outside: <body>";
    if (!d.contains(a)) return `outside: ${a.getAttribute("data-testid") ?? a.tagName.toLowerCase()}`;
    const items = [...d.querySelectorAll(selector)];
    if (a === items[0]) return "first control";
    if (a === items.at(-1)) return "last control";
    return a.getAttribute("data-testid") ?? a.tagName.toLowerCase();
  }, FOCUSABLE);

/** Presses Tab (Shift+Tab with `back`) until `target` has focus. */
async function tabTo(page: Page, target: Locator, { back = false, max = 60 } = {}) {
  await target.waitFor();
  for (let i = 0; i <= max; i++) {
    if (await target.evaluate((el) => el === document.activeElement)) return;
    await page.keyboard.press(back ? "Shift+Tab" : "Tab");
  }
  throw new Error(`${back ? "Shift+Tab" : "Tab"} never reached ${target}`);
}

async function unlock(page: Page) {
  const field = page.getByTestId("lock-field");
  await tabTo(page, field);
  await page.keyboard.type(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}

/** A pay link fills Pay ▸ To an address with 0.5 to PAYEE; Review is reached with Tab and pressed. */
async function review(page: Page) {
  await page.evaluate((address) => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address, amount: 50_000_000, label: null, message: null }, username: null }]), PAYEE);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.50000000");
  const next = page.getByTestId("pay-continue");
  await expect(next).toBeEnabled();
  await tabTo(page, next);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
}

/** Review, the passphrase and Enter: the payment is sent, its broadcast held until `release`. */
async function sendHeld(page: Page) {
  await review(page);
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeFocused();
  await page.keyboard.type(FIXTURE_PASSPHRASE);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("draft_broadcast"))).toBe(true);
  return () => release.evaluate((r) => r());
}

const broadcasts = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_broadcast").length);

test("keyboard only: Tab and Shift+Tab from a sheet that has focus and nothing to press keep focus on it", async ({ page }) => {
  const errors = await open(page, "locked");
  await unlock(page);
  const release = await sendHeld(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await unlock(page);

  const sending = page.getByTestId("pay-sending");
  await expect(sending).toBeFocused();
  for (const key of ["Shift+Tab", "Tab", "Tab", "Shift+Tab"]) {
    await page.keyboard.press(key);
    await expect.poll(() => focusIn(sending), key).toBe("dialog");
  }
  await release();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(errors).toEqual([]);
});

for (const [scenario, wallet] of [["watch-only", "a watch-only wallet"], ["funded", "a wallet that asks for its passphrase"]])
  test(`keyboard only: a failed Create Unsigned gives the focus back to Create Unsigned (${wallet})`, async ({ page }) => {
    const errors = await open(page, scenario);
    await review(page);
    const sheet = page.getByTestId("pay-confirm");
    await expect(page.getByTestId("pay-confirm-send")).toHaveCount(scenario === "watch-only" ? 0 : 1);
    const create = page.getByTestId("pay-confirm-unsigned");
    await tabTo(page, create);
    const release = await page.evaluateHandle(() => {
      window.__dwFixture!.fail("draft_create_unsigned", "rpc");
      return window.__dwFixture!.hold("draft_create_unsigned");
    });
    await page.keyboard.press("Enter");
    await expect(create).toBeDisabled();
    await expect.poll(() => focusIn(sheet)).toBe("dialog");
    await release.evaluate((r) => r());
    await expect(page.getByTestId("pay-confirm-error")).not.toBeEmpty();
    await expect(create).toBeEnabled();
    await expect(create).toBeFocused();
    // Enter tries again from there.
    await page.evaluate(() => window.__dwFixture!.fail("draft_create_unsigned", null));
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("tools-tabs-psbt")).toHaveAttribute("aria-selected", "true");
    expect(errors).toEqual([]);
  });

const frames = (page: Page) => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

/** Try again pressed with Enter, its broadcast held until the returned release, which waits for the retry to end. */
async function retryHeld(page: Page, result: Locator, answer: "noPeers" | "ok") {
  await page.evaluate((a) => void (window.__dwFixture!.world.send.broadcast = a), answer);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  const before = await broadcasts(page);
  const tries = Number(await page.getByTestId("pay-result").locator("[data-retries]").getAttribute("data-retries"));
  await page.keyboard.press("Enter");
  await expect.poll(() => broadcasts(page)).toBe(before + 1);
  await expect(page.getByTestId("pay-result-retry")).toBeDisabled();
  await expect.poll(() => focusIn(result)).toBe("dialog");
  return async () => {
    await release.evaluate((r) => r());
    if (answer !== "ok") {
      await expect(page.getByTestId("pay-result").locator("[data-retries]")).toHaveAttribute("data-retries", String(tries + 1));
      await expect(page.getByTestId("pay-result-retry")).toBeEnabled();
      await frames(page);
    }
  };
}

test("keyboard only: Try again after an unknown outcome keeps focus in the sheet, gives it back to Try again, and leaves it where the user moved it", async ({ page }) => {
  const errors = await open(page, "funded");
  await world(page, (w) => (w.send.broadcast = "unknown"));
  await (await sendHeld(page))();
  const result = page.getByTestId("pay-result");
  const retry = page.getByTestId("pay-result-retry");
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await expect(retry).toBeFocused();

  // A retry that fails is still unknown. With focus left on the dialog, it goes back to Try again.
  let done = await retryHeld(page, result, "noPeers");
  await done();
  await expect(retry).toBeFocused();

  // Shift+Tab while it runs goes to the sheet's last control, and focus stays there when it ends.
  done = await retryHeld(page, result, "noPeers");
  await page.keyboard.press("Shift+Tab");
  await expect.poll(() => focusIn(result)).toBe("last control");
  const moved = await page.evaluate(() => document.activeElement?.outerHTML);
  await done();
  expect(await page.evaluate(() => document.activeElement?.outerHTML)).toBe(moved);

  // Tab while it runs goes to the first.
  await tabTo(page, retry);
  done = await retryHeld(page, result, "ok");
  await page.keyboard.press("Tab");
  await expect.poll(() => focusIn(result)).toBe("first control");
  await done();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(errors).toEqual([]);
});

async function openTxDetail(page: Page) {
  await tabTo(page, page.getByTestId("sidebar-activity"));
  await page.keyboard.press("Enter");
  await tabTo(page, page.locator("[data-txid]").first());
  await page.keyboard.press("Enter");
  const details = page.getByTestId("tx-detail-technical");
  await expect(details).not.toHaveAttribute("open");
  return { sheet: page.getByTestId("tx-detail"), summary: details.locator("summary") };
}

test("keyboard only: Tab past a closed disclosure at the end of a sheet wraps to its first control", async ({ page }) => {
  const errors = await open(page, "funded");
  const { sheet, summary } = await openTxDetail(page);
  await tabTo(page, summary);
  await page.keyboard.press("Tab");
  await expect.poll(() => focusIn(sheet)).toBe("first control");
  await page.keyboard.press("Shift+Tab");
  await expect(summary).toBeFocused();
  expect(errors).toEqual([]);
});

test("keyboard only: without checkVisibility, Tab past a closed disclosure still wraps to the first control", async ({ page }) => {
  await page.addInitScript(() => void delete (Element.prototype as Partial<Element>).checkVisibility);
  const errors = await open(page, "funded");
  const { sheet, summary } = await openTxDetail(page);
  await tabTo(page, summary);
  await page.keyboard.press("Tab");
  await expect.poll(() => focusIn(sheet)).toBe("first control");
  expect(errors).toEqual([]);
});

for (const [what, html] of [
  ["a hidden button", '<button type="button" style="visibility: hidden">x</button>'],
  ["a disabled select", "<select disabled><option>x</option></select>"],
  ["a disabled textarea", "<textarea disabled></textarea>"],
  ["a button in a disabled fieldset", '<fieldset disabled><button type="button">x</button></fieldset>'],
  ["an inert button", '<div inert><button type="button">x</button></div>'],
])
  test(`keyboard only: Tab from a sheet's last control wraps to its first when ${what} that takes no focus follows it`, async ({ page }) => {
    const errors = await open(page, "funded");
    const { sheet, summary } = await openTxDetail(page);
    await sheet.locator(".sheet-body").evaluate((body, h) => body.insertAdjacentHTML("beforeend", h), html);
    await tabTo(page, summary);
    await page.keyboard.press("Tab");
    await expect.poll(() => focusIn(sheet)).toBe("first control");
    expect(errors).toEqual([]);
  });

test("keyboard only: a control that handles Tab itself keeps it", async ({ page }) => {
  const errors = await open(page, "funded");
  const { sheet } = await openTxDetail(page);
  await sheet.locator(".sheet-body").evaluate((body) => {
    const input = document.createElement("input");
    input.dataset.testid = "own-tab";
    input.addEventListener("keydown", (e) => e.key === "Tab" && e.preventDefault());
    body.append(input);
  });
  const own = page.getByTestId("own-tab");
  await tabTo(page, own);
  await page.keyboard.press("Tab");
  await expect(own).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(own).toBeFocused();
  expect(errors).toEqual([]);
});
