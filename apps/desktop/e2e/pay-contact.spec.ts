import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP3-06: Pay ▸ To a contact on the fixture backend, whose contact payments follow DP3-01's
// engine rules (src/fixture/payContact.test.ts): the frequent strip and the picker, U-03's confirm
// and result sheets with the contact, "Accept and pay" under one passphrase, the lock banner, and
// the contact pane's hand-off. Tests are named by parity ID: IOS-050 (pay a contact), IOS-074 (the
// pane's Pay and Accept and pay).

const world = <T>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

const GRACE = "Grace Hopper";

async function openPay(page: Page, scenario = "dashpay-registered", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await expect(page.getByTestId("pay-tabs-contact")).toHaveAttribute("aria-selected", "true");
  return errors;
}

const pickerRow = (page: Page, name: string) => page.getByTestId("pay-picker-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });

async function fillAmount(page: Page, amount: string) {
  await page.getByTestId("pay-amount").fill(amount);
  await page.getByTestId("pay-fee-line").waitFor();
  const review = page.getByTestId("pay-continue");
  await review.and(page.locator(":enabled")).waitFor();
  await review.click();
  await page.getByTestId("pay-confirm").waitFor();
}

async function send(page: Page) {
  await page.getByTestId("pay-confirm-passphrase").fill("demo");
  await page.getByTestId("pay-confirm-send").click();
}

test("IOS-050 pays a frequent contact; the confirm sheet names them and Activity attributes it", async ({ page }) => {
  const errors = await openPay(page);
  await expect(page.getByTestId("pay-frequent-chip").locator(".pay-frequent-name")).toHaveText(["Bob Builder", "dave"]);
  await page.getByTestId("pay-frequent-chip").first().click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  await expect(page.getByTestId("pay-payee")).toContainText("@bob");
  await expect(page.getByTestId("pay-banner")).toHaveCount(0);
  await page.getByTestId("pay-note").fill("Lunch");
  await fillAmount(page, "0.01");
  const sheet = page.getByTestId("pay-confirm");
  await expect(sheet.getByTestId("pay-confirm-contact")).toContainText("Bob Builder");
  await expect(sheet.getByTestId("pay-confirm-contact")).toContainText("@bob");
  await expect(sheet.locator(".contact-avatar")).toHaveText("BB");
  await expect(sheet).toContainText("Lunch");
  // The contact's address exists only once prepared: no unsigned contact payment.
  await expect(sheet.getByTestId("pay-confirm-unsigned")).toHaveCount(0);
  await expect(sheet.getByTestId("pay-confirm-send")).toHaveText("Send");
  await send(page);
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await page.getByTestId("pay-result-show").click();
  await expect(page.getByTestId("tx-detail-contact")).toContainText("Bob Builder");
  // The fixture recorded the contact's payment at its reserved address, attributed to Bob.
  expect(await world(page, (w) => w.wallets[0].rows[0].counterparty?.username)).toBe("bob");
  expect(errors).toEqual([]);
});

test("IOS-050 the picker searches the contacts and requests, and Change goes back to it", async ({ page }) => {
  await openPay(page);
  await expect(page.getByTestId("pay-picker-contacts").locator(".contact-row-name")).toContainText(["Bob Builder", "Carol"]);
  await expect(page.getByTestId("pay-picker-requests").locator(".contact-row-name")).toHaveText(["heidi", GRACE]);
  // Requests are newest first (the engine's order); Pending and Hidden are not payable from here.
  await expect(pickerRow(page, "Frank")).toHaveCount(0);
  await expect(pickerRow(page, "Judy")).toHaveCount(0);
  await expect(pickerRow(page, "dave")).toContainText("Payments cannot be sent right now");
  await page.getByTestId("pay-contact-search").locator("input").fill("car");
  await expect(page.getByTestId("pay-picker-row")).toHaveCount(1);
  // A search hides the frequent strip.
  await expect(page.getByTestId("pay-frequent")).toHaveCount(0);
  await pickerRow(page, "Carol").click();
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "note");
  await expect(page.getByTestId("pay-amount")).toBeVisible();
  await page.getByTestId("pay-payee-change").click();
  await expect(page.getByTestId("pay-contact")).toBeVisible();
  await page.getByTestId("pay-contact-search").locator("input").fill("zzz");
  await expect(page.getByTestId("pay-picker-none")).toBeVisible();
});

test("IOS-050 Accept and pay accepts the request and pays it under one passphrase", async ({ page }) => {
  const errors = await openPay(page);
  await pickerRow(page, GRACE).click();
  await expect(page.getByTestId("pay-banner")).toContainText("Accept and pay accepts it and pays in one step.");
  await fillAmount(page, "0.02");
  const sheet = page.getByTestId("pay-confirm");
  await expect(sheet).toContainText(`Accept ${GRACE}'s contact request and send this payment?`);
  await expect(sheet.getByTestId("pay-confirm-accept")).toContainText("Accepted with this payment");
  await expect(sheet.getByTestId("pay-confirm-send")).toHaveText("Accept and pay");
  await send(page);
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expect(page.getByTestId("pay-result-note")).toHaveText(`You and ${GRACE} are now contacts.`);
  // One credential: no grant sheet asked again; the lease ended with the broadcast.
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  expect(await world(page, (w) => w.dashpay.contacts.find((c) => c.summary.username === "grace")?.summary.relation)).toBe("contact");
  expect(await world(page, (w) => w.leases.filter((l) => l.flow === "acceptAndPay").map((l) => l.state.kind))).toEqual(["ended"]);
  await page.getByTestId("pay-result-close").click();
  // Paying again is a plain payment (D11).
  await fillAmount(page, "0.01");
  await expect(page.getByTestId("pay-confirm-send")).toHaveText("Send");
  await expect(page.getByTestId("pay-confirm-accept")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("IOS-050 C8: an accept that went through with a payment that did not says both; Back pays without accepting again", async ({ page }) => {
  await openPay(page);
  await world(page, (w) => (w.send.broadcast = "noPeers"));
  await pickerRow(page, GRACE).click();
  await fillAmount(page, "0.02");
  await send(page);
  await expect(page.getByTestId("pay-result-failed")).toBeVisible();
  await expect(page.getByTestId("pay-result-text")).toHaveText(`You and ${GRACE} are now contacts. The payment was not sent: No connection to the Dash network. The payment was not sent.`);
  expect(await world(page, (w) => w.leases.filter((l) => l.flow === "acceptAndPay").map((l) => l.state.kind))).toEqual(["ended"]);
  await page.getByTestId("pay-result-back").click();
  await world(page, (w) => (w.send.broadcast = "ok"));
  await expect(page.getByTestId("pay-amount")).toHaveValue(/^0\.02/);
  const review = page.getByTestId("pay-continue");
  await review.and(page.locator(":enabled")).waitFor();
  await review.click();
  await expect(page.getByTestId("pay-confirm-send")).toHaveText("Send");
  await send(page);
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expect(page.getByTestId("pay-result-note")).toHaveCount(0);
});

test("IOS-050 an accept refused pays nothing and says why", async ({ page }) => {
  await openPay(page);
  await world(page, (w) => (w.dashpay.identities[0].balance = "0"));
  await pickerRow(page, GRACE).click();
  await fillAmount(page, "0.02");
  const rows = await world(page, (w) => w.wallets[0].rows.length);
  await send(page);
  await expect(page.getByTestId("pay-result-failed")).toBeVisible();
  await expect(page.getByTestId("pay-result-text")).toHaveText("Not enough credits. Top up your credits and try again. Nothing was paid.");
  expect(await world(page, (w) => w.wallets[0].rows.length)).toBe(rows);
  expect(await world(page, (w) => w.dashpay.contacts.find((c) => c.summary.username === "grace")?.summary.relation)).toBe("pendingIncoming");
  expect(await world(page, (w) => w.leases.filter((l) => l.flow === "acceptAndPay").map((l) => l.state.kind))).toEqual(["ended"]);
  // A wrong passphrase stays on the sheet: nothing was accepted.
  await page.getByTestId("pay-result-back").click();
  const review = page.getByTestId("pay-continue");
  await review.and(page.locator(":enabled")).waitFor();
  await review.click();
  await page.getByTestId("pay-confirm-passphrase").fill("wrong");
  await page.getByTestId("pay-confirm-send").click();
  await expect(page.getByTestId("pay-confirm-error")).toHaveText("The passphrase you entered was incorrect.");
});

test("IOS-050 Accept and pay when another device accepted first pays under the same lease", async ({ page }) => {
  await openPay(page);
  await pickerRow(page, GRACE).click();
  await fillAmount(page, "0.02");
  await expect(page.getByTestId("pay-confirm-send")).toHaveText("Accept and pay");
  // Accepted elsewhere while the sheet was open: the engine says `contact.already_contact`.
  await world(page, (w) => {
    const grace = w.dashpay.contacts.find((c) => c.summary.username === "grace")!;
    grace.summary = { ...grace.summary, relation: "contact" };
  });
  await send(page);
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expect(page.getByTestId("pay-result-note")).toHaveText(`You and ${GRACE} are now contacts.`);
  expect(await world(page, (w) => w.leases.filter((l) => l.flow === "acceptAndPay").map((l) => l.state.kind))).toEqual(["ended"]);
});

test("IOS-050 the lock banner: an unknown outcome locks the contact until Check again finds the payment", async ({ page }) => {
  await openPay(page);
  await page.getByTestId("pay-frequent-chip").first().click();
  await world(page, (w) => (w.send.broadcast = "unknown"));
  await fillAmount(page, "0.01");
  await send(page);
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await page.getByTestId("pay-result-close").click();
  const banner = page.getByTestId("pay-banner");
  await expect(banner).toHaveAttribute("data-kind", "paymentLock");
  await expect(banner).toContainText("A payment to this contact may have been sent.");
  await expect(page.getByTestId("pay-amount")).toHaveCount(0);
  await page.getByTestId("pay-banner-check").click();
  await expect(page.getByTestId("pay-lock-resolve")).toHaveText("The payment was sent. You can pay this contact again.");
  await expect(page.getByTestId("pay-banner")).toHaveCount(0);
  await expect(page.getByTestId("pay-amount")).toBeVisible();
  // A new payment's lock is not answered by the old Check again.
  await fillAmount(page, "0.01");
  await send(page);
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await page.getByTestId("pay-result-close").click();
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "paymentLock");
  await expect(page.getByTestId("pay-contact-form")).not.toContainText("The payment was sent.");
});

test("IOS-050 a scenario's lock: Check again says unknown, then not sent; the broken channel shows next", async ({ page }) => {
  await openPay(page);
  await pickerRow(page, "dave").click();
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "paymentLock");
  await page.getByTestId("pay-banner-check").click();
  await expect(page.getByTestId("pay-lock-resolve")).toHaveText("Still not known. Check again once the wallet has synced.");
  await world(page, (w) => (w.send.lockEvidence = "notSent"));
  await page.getByTestId("pay-banner-check").click();
  await expect(page.getByTestId("pay-lock-resolve")).toHaveText("The payment was not sent. You can pay this contact again.");
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "broken");
  await page.getByTestId("pay-banner-show").waitFor({ state: "detached" });
});

test("IOS-050 unlocked for mixing only: the banner offers the full unlock", async ({ page }) => {
  await openPay(page, "dashpay-contacts-mixing-only");
  await pickerRow(page, "Bob Builder").click();
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "mixingOnly");
  await page.getByTestId("pay-banner-unlock").click();
  await expect(page.getByTestId("unlock-sheet")).toBeVisible();
});

test("IOS-050 without an identity the tab says why and offers Join and To an address", async ({ page }) => {
  await openPay(page, "dashpay-no-identity");
  await expect(page.getByTestId("pay-contact-none")).toContainText("Join DashPay");
  await page.getByTestId("pay-contact-to-address").click();
  await expect(page.getByTestId("pay-tabs-address")).toHaveAttribute("aria-selected", "true");
});

test("IOS-074 the pane's Pay and Accept and pay open the tab on that contact", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-registered", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  const row = (name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
  await row("Bob Builder").click();
  await page.getByTestId("contact-pay").click();
  await expect(page.getByTestId("page-pay")).toBeVisible();
  await expect(page.getByTestId("pay-payee-name")).toHaveText("Bob Builder");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  await row(GRACE).click();
  await page.getByTestId("contact-accept-pay").click();
  await expect(page.getByTestId("pay-payee-name")).toHaveText(GRACE);
  await expect(page.getByTestId("pay-banner")).toContainText("Accept and pay");
  // dave's lock: the pane says so and Pay leads to the banner.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  await row("dave").click();
  await expect(page.getByTestId("contact-pane-note")).toHaveText("A payment to this contact may have been sent. Pay shows it and checks it again.");
  await page.getByTestId("contact-pay").click();
  await expect(page.getByTestId("pay-banner")).toHaveAttribute("data-kind", "paymentLock");
});

const STATES: Record<string, { scenario?: string; go: (page: Page) => Promise<unknown> }> = {
  chooser: { go: (page) => page.getByTestId("pay-frequent").waitFor() },
  form: {
    go: async (page) => {
      await pickerRow(page, "Carol").click();
      await page.getByTestId("pay-amount").fill("0.01");
      await page.getByTestId("pay-fee-line").waitFor();
    },
  },
  lock: { go: async (page) => (await pickerRow(page, "dave").click(), page.getByTestId("pay-banner-check").waitFor()) },
  "accept-confirm": {
    go: async (page) => {
      await pickerRow(page, GRACE).click();
      await fillAmount(page, "0.02");
      await page.getByTestId("pay-confirm-passphrase").waitFor();
    },
  },
  none: { scenario: "dashpay-no-identity", go: (page) => page.getByTestId("pay-contact-none").waitFor() },
};

for (const theme of THEMES) {
  for (const [name, { scenario, go }] of Object.entries(STATES)) {
    test(`pay-contact ${name} ${theme}: 0 serious axe issues`, async ({ page }, info) => {
      const errors = await openPay(page, scenario, theme);
      await go(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-pay-contact-${name}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
