import { expect, test, type Page } from "@playwright/test";
import { MARKER_SCRIPT } from "../e2e-tauri/atspi-marker";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl } from "./helpers";

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";

async function pay(page: Page) {
  await page.goto(fixtureUrl("dashpay-registered", "light"));
  await page.getByTestId("sidebar-pay").click();
  return page.getByRole("combobox", { name: "Pay to" });
}

test("a pasted payment URI reviews and sends the exact address and amount", async ({ page }) => {
  const field = await pay(page);
  await field.fill(`dash:${ADDRESS}?amount=0.02500001&label=Rent`);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.02500001");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.02500001");
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_broadcast"))).toHaveLength(1);
});

test("keyboard suggestions lead to the contact send, with no layout shift", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  const before = await amount.boundingBox();
  await field.fill("Bob");
  await expect(page.getByRole("option").first()).toContainText("Bob Builder");
  expect(await amount.boundingBox()).toEqual(before);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await field.press("ArrowDown");
  await expect(field).toHaveAttribute("aria-activedescendant", /option/);
  await field.press("Escape");
  await expect(field).toHaveAttribute("aria-expanded", "false");
  await field.press("ArrowDown");
  await field.press("Enter");
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  await amount.fill("0.01");
  await page.getByTestId("pay-note").fill("Lunch");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await expect(page.getByTestId("pay-confirm")).toContainText("Lunch");
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_broadcast"))).toHaveLength(1);
});

test("Contacts opens the list and clearing a picked contact returns to the same field", async ({ page }) => {
  const field = await pay(page);
  await page.getByRole("button", { name: "Contacts", exact: true }).click();
  await expect(page.getByRole("listbox")).toBeVisible();
  await page.getByRole("option", { name: /Carol/ }).click();
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Carol");
  await page.getByTestId("pay-payee-change").click();
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("");
  await expect(page.getByTestId("pay-address-book")).toBeVisible();
});

test("pasting a QR image resolves in the same field", async ({ page }) => {
  const field = await pay(page);
  await field.evaluate((el, address) => {
    const data = new DataTransfer();
    data.items.add(new File([`fixture-qr:\ndash:${address}?amount=0.25`], "fixture.png", { type: "image/png" }));
    el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, ADDRESS);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(field).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.25000000");
});

test("changing a quoted recipient requires a fresh estimate before review", async ({ page }) => {
  const field = await pay(page);
  const review = page.getByTestId("pay-continue");
  await field.fill(ADDRESS);
  await page.getByTestId("pay-amount").fill("0.02");
  await expect(review).toBeEnabled();
  await page.evaluate(() => {
    (window as Window & { releaseEstimate?: () => void }).releaseEstimate = window.__dwFixture!.hold("draft_estimate");
  });

  const next = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await field.fill(next);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(review).toBeDisabled();
  await expect(page.getByTestId("pay-fee-line")).toHaveCount(0);
  await page.getByTestId("pay-amount").press("Enter");
  await expect(page.getByTestId("pay-confirm")).toHaveCount(0);

  await page.evaluate(() => (window as Window & { releaseEstimate?: () => void }).releaseEstimate?.());
  await review.click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(next);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.02");
});

test("a late payment link cannot replace a contact selected while it was resolving", async ({ page }) => {
  const field = await pay(page);
  await page.evaluate(() => {
    (window as Window & { releaseLink?: () => void }).releaseLink = window.__dwFixture!.hold("route_link");
  });
  await field.fill(`dash:${ADDRESS}?amount=0.25&label=Rent`);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("route_link"))).toBe(true);
  await field.fill("Bob");
  await page.getByRole("option", { name: /Bob Builder/ }).click();
  await page.evaluate(() => (window as Window & { releaseLink?: () => void }).releaseLink?.());

  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  const amount = page.getByTestId("pay-amount");
  await expect(amount).toHaveValue("");
  await amount.fill("0.01");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.01");
});

test("an invalid address reports the existing inline error without moving the form", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  await expect(amount).toBeVisible();
  const before = await amount.boundingBox();
  await field.fill(ADDRESS.slice(0, -1) + "0");
  await expect(field).toHaveAccessibleDescription("The recipient address is not valid. Please recheck.");
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  expect(await amount.boundingBox()).toEqual(before);
});

test("a DashPay user link resolves to the verified contact identity", async ({ page }) => {
  const field = await pay(page);
  const identity = await page.evaluate(() =>
    window.__dwFixture!.world.dashpay.contacts.find((c) => c.summary.username === "bob")!.summary.contact,
  );
  await field.fill(`dashpay://user?id=${identity}&username=bob`);
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("@bob");
  const calls = await page.evaluate(() => window.__dwFixture!.calls);
  expect(calls).toContain("dashpay_verify_scanned");
  expect(calls).toContain("draft_set_contact_recipient");
  expect(calls).not.toContain("draft_set_recipients");
});

test("an address resolving preserves the amount typed before or during resolution", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  const review = page.getByTestId("pay-continue");

  await amount.click();
  await page.keyboard.type("0.03700001");
  await expect(amount).toHaveValue("0.03700001");
  await field.click();
  await page.keyboard.type(ADDRESS);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(amount).toHaveValue("0.03700001");
  await review.click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.03700001");
  await page.getByTestId("pay-confirm-cancel").click();

  const next = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  const classifications = () => page.evaluate(() => window.__dwFixture!.calls.filter((call) => call === "classify_address").length);
  const before = await classifications();
  await page.evaluate(() => {
    (window as Window & { releaseRecipient?: () => void }).releaseRecipient = window.__dwFixture!.hold("classify_address");
  });
  await page.getByTestId("pay-recipient-clear").click();
  await expect(field).toBeFocused();
  await page.keyboard.type(next);
  await expect.poll(classifications).toBeGreaterThan(before);
  await expect(field).toHaveValue(next);
  await expect(review).toBeDisabled();
  await amount.click();
  await amount.selectText();
  await page.keyboard.type("0.04100001");
  await expect(amount).toHaveValue("0.04100001");
  await page.evaluate(() => (window as Window & { releaseRecipient?: () => void }).releaseRecipient?.());
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(amount).toHaveValue("0.04100001");
  await review.click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(next);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.04100001");
});

test("a payment link with an amount preserves a typed amount, and picking a contact resets a payment-link amount", async ({ page }) => {
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");
  const review = page.getByTestId("pay-continue");

  // Type an amount first, then paste a payment URI that carries its own amount.
  await amount.fill("0.03700001");
  await field.fill(`dash:${ADDRESS}?amount=0.25&label=Rent`);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
  await expect(amount).toHaveValue("0.03700001");
  await review.click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.03700001");
  await page.getByTestId("pay-confirm-cancel").click();

  // Clear amount and recipient, paste a URI while holding route_link, then type an amount before it resolves.
  const next = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await amount.fill("");
  await page.getByTestId("pay-recipient-clear").click();
  const routeCalls = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "route_link").length);
  const beforeRoute = await routeCalls();
  await page.evaluate(() => {
    (window as Window & { releaseLink?: () => void }).releaseLink = window.__dwFixture!.hold("route_link");
  });
  await field.fill(`dash:${next}?amount=0.5`);
  await expect.poll(routeCalls).toBeGreaterThan(beforeRoute);
  await amount.fill("0.04100001");
  await page.evaluate(() => (window as Window & { releaseLink?: () => void }).releaseLink?.());
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(amount).toHaveValue("0.04100001");

  // Clear amount and recipient, let a URI fill the untouched amount, then pick a contact: the link amount resets.
  await amount.fill("");
  await page.getByTestId("pay-recipient-clear").click();
  await field.fill(`dash:${ADDRESS}?amount=0.25&label=Rent`);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(amount).toHaveValue("0.25000000");
  await page.getByTestId("pay-contacts").click();
  await page.getByRole("option", { name: /Bob Builder/ }).click();
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
  await expect(amount).toHaveValue("");
});

test("an old send finishing after leaving Pay does not clear a contact picked in a new Pay", async ({ page }) => {
  const field = await pay(page);
  await field.fill(ADDRESS);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await page.getByTestId("pay-amount").fill("0.02");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);

  const rowsBefore = await page.evaluate(() => window.__dwFixture!.world.wallets[0].rows.length);
  await page.evaluate(() => {
    (window as Window & { releaseBroadcast?: () => void }).releaseBroadcast = window.__dwFixture!.hold("draft_broadcast");
  });
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("draft_broadcast"))).toBe(true);

  // Leave Pay, then open a new Pay for a contact and open Review while the first send is still in flight.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  await page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: "Bob Builder" }) }).click();
  await page.getByTestId("contact-pay").click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  await page.getByTestId("pay-amount").fill("0.01");
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");

  // Let the first send finish; the newly picked contact and its confirmation payee must stay.
  await page.evaluate(() => (window as Window & { releaseBroadcast?: () => void }).releaseBroadcast?.());
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.wallets[0].rows.length)).toBeGreaterThan(rowsBefore);
  await expect(page.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await page.getByTestId("pay-confirm-cancel").click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  await expect(page.getByTestId("pay-recipient-chip")).toContainText("Bob Builder");
});

test("Contacts browse shows updating while contacts sync instead of No contacts yet", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-contacts-updating", "light"));
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-contacts").click();
  await expect(page.getByTestId("pay-picker-updating")).toHaveText("Your contacts are being updated.");
  await expect(page.getByTestId("pay-picker-empty")).toHaveCount(0);
  await expect(page.getByTestId("pay-recipient-popover")).not.toContainText("No contacts yet");
});

test("a late DashPay user link cannot replace a recipient entered while it was resolving", async ({ page }) => {
  const field = await pay(page);
  const identity = await page.evaluate(() =>
    window.__dwFixture!.world.dashpay.contacts.find((c) => c.summary.username === "bob")!.summary.contact,
  );
  await page.evaluate(() => {
    (window as Window & { releaseScan?: () => void }).releaseScan = window.__dwFixture!.hold("dashpay_verify_scanned");
  });
  await field.fill(`dashpay://user?id=${identity}&username=bob`);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("dashpay_verify_scanned"))).toBe(true);
  await field.fill(ADDRESS);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(field).toHaveValue(ADDRESS);
  await page.evaluate(() => (window as Window & { releaseScan?: () => void }).releaseScan?.());
  await expect(field).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-payee-name")).toHaveCount(0);
});

test("a Core Max result is discarded when the recipient resolves to Platform, and kept when it resolves to Core", async ({ page }) => {
  const PLATFORM = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
  const field = await pay(page);
  const amount = page.getByTestId("pay-amount");

  // Case 1: Core Max in flight while Platform address resolves -> late Core Max is discarded.
  const classifyCalls = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "classify_address").length);
  const maxCalls = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "max_spendable").length);
  const c0 = await classifyCalls();
  const m0 = await maxCalls();
  await page.evaluate(() => {
    const w = window as Window & { releaseClassify?: () => void; releaseMax?: () => void };
    w.releaseClassify = window.__dwFixture!.hold("classify_address");
    w.releaseMax = window.__dwFixture!.hold("max_spendable");
  });
  await field.fill(PLATFORM);
  await expect.poll(classifyCalls).toBeGreaterThan(c0);
  await page.getByTestId("pay-max").click();
  await expect.poll(maxCalls).toBeGreaterThan(m0);
  await page.evaluate(() => (window as Window & { releaseClassify?: () => void }).releaseClassify?.());
  await expect(page.getByTestId("platform-send-intro")).toBeVisible();
  await page.evaluate(() => (window as Window & { releaseMax?: () => void }).releaseMax?.());
  await expect(amount).toHaveValue("");

  // Case 2: Core Max finishes before Platform resolution completes -> Platform resolution clears Core Max.
  await page.getByTestId("pay-recipient-clear").click();
  const c1 = await classifyCalls();
  await page.evaluate(() => {
    (window as Window & { releaseClassify?: () => void }).releaseClassify = window.__dwFixture!.hold("classify_address");
  });
  await field.fill(PLATFORM);
  await expect.poll(classifyCalls).toBeGreaterThan(c1);
  await page.getByTestId("pay-max").click();
  await expect(amount).not.toHaveValue("");
  await page.evaluate(() => (window as Window & { releaseClassify?: () => void }).releaseClassify?.());
  await expect(page.getByTestId("platform-send-intro")).toBeVisible();
  await expect(amount).toHaveValue("");

  // Case 3: Core Max in flight while Core address resolves -> Core Max is accepted.
  await page.getByTestId("pay-recipient-clear").click();
  const c2 = await classifyCalls();
  const m2 = await maxCalls();
  await page.evaluate(() => {
    const w = window as Window & { releaseClassify?: () => void; releaseMax?: () => void };
    w.releaseClassify = window.__dwFixture!.hold("classify_address");
    w.releaseMax = window.__dwFixture!.hold("max_spendable");
  });
  await field.fill(ADDRESS);
  await expect.poll(classifyCalls).toBeGreaterThan(c2);
  await page.getByTestId("pay-max").click();
  await expect.poll(maxCalls).toBeGreaterThan(m2);
  await page.evaluate(() => (window as Window & { releaseClassify?: () => void }).releaseClassify?.());
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await page.evaluate(() => (window as Window & { releaseMax?: () => void }).releaseMax?.());
  await expect(amount).not.toHaveValue("");
});

test("link-derived amount, label and message clear when the recipient changes, while typed ones stay", async ({ page }) => {
  const field = await pay(page);
  const next = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await field.fill(`dash:${ADDRESS}?amount=0.25&label=Rent&message=June`);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.25000000");
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
  await expect(page.getByTestId("pay-message")).toHaveValue("June");

  await page.getByTestId("pay-message").fill("Personal");
  await field.fill(next);
  await expect(page.getByTestId("pay-recipient-chip")).toBeVisible();
  await expect(page.getByTestId("pay-amount")).toHaveValue("");
  await expect(page.getByTestId("pay-label")).toHaveValue("");
  await expect(page.getByTestId("pay-message")).toHaveValue("Personal");
});

test("Change on a resolved non-contact user link clears Pay to instead of reopening the card", async ({ page }) => {
  const field = await pay(page);
  const identity = await page.evaluate(() =>
    window.__dwFixture!.world.dashpay.directory.find((u) => u.username === "mallory")!.identity,
  );
  await field.fill(`dashpay://user?id=${identity}&username=mallory`);
  await expect(page.getByTestId("pay-found-name")).toHaveText("Mallory");
  await page.getByTestId("pay-found-back").click();
  await expect(page.getByTestId("pay-found")).toHaveCount(0);
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("");
});

test("Escape closes an empty Contacts popup even when a contact is already picked", async ({ page }) => {
  await pay(page);
  await page.getByTestId("pay-contacts").click();
  await page.getByRole("option", { name: /Bob Builder/ }).click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");

  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.dashpay.contacts = [];
    f.world.dashpay.contactsUpdating = "noSnapshot";
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "contacts", identity: f.world.dashpay.identities[0].identity } });
  });
  const contactsButton = page.getByTestId("pay-contacts");
  await contactsButton.click();
  await expect(page.getByTestId("pay-picker-updating")).toBeVisible();
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("pay-recipient-popover")).toHaveCount(0);
  await expect(contactsButton).toBeFocused();
});

test("a 25+ character contact alias matches local contacts instead of failing as an invalid address", async ({ page }) => {
  const alias = "AlexandertheGreatOfMacedon";
  const field = await pay(page);
  await page.evaluate((nextAlias) => {
    const f = window.__dwFixture!;
    const bob = f.world.dashpay.contacts.find((c) => c.summary.username === "bob")!;
    bob.summary = { ...bob.summary, alias: nextAlias };
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "contacts", identity: f.world.dashpay.identities[0].identity } });
  }, alias);
  await field.fill(alias);
  await expect(page.getByRole("option").first()).toContainText(alias);
  await page.getByRole("option").first().click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText(alias);
});

test("AT-SPI Pay contacts walk finds a marker while waitingForSync", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-waiting-for-sync", "light"));
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-contacts").click();
  await expect(page.getByTestId("pay-contact-none")).toBeVisible();
  const marker = await page.evaluate(
    ([script, selector]) => (new Function(script) as (sel: string) => unknown)(selector),
    [MARKER_SCRIPT, '[data-testid="pay-contact-none"]'] as const,
  );
  expect(marker).toEqual(["PARAGRAPH", "You can join DashPay once the wallet has synced."]);
});
