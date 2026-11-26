import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { payFromDashWallet } from "./dashpay";
import { authorize, fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// PRODUCT #4, the DashPay welcome moment: a registration's success (R9) congratulates the new
// @username and offers three next steps, each leading to its place, and a quiet Go to wallet.

const LABEL = "alice-77";
const screen = (page: Page) => page.getByTestId("registration-screen");

/** A new username from the Join page, run to its end: the welcome. */
async function toWelcome(page: Page, theme: Theme = "light", whileRunning?: () => Promise<void>) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-no-identity", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await page.getByTestId("join-create").click();
  await payFromDashWallet(page);
  await page.getByTestId("registration-name-field").fill(LABEL);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await page.getByTestId("registration-pay").click();
  await authorize(page);
  await expect(screen(page)).toHaveAttribute("data-kind", "progress");
  await whileRunning?.();
  await page.evaluate(() => {
    const d = window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft;
    while (window.__dwFixture!.advanceRegistration(d));
  });
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  return errors;
}

/** A step taken closes the welcome for good: Join DashPay then says you joined. */
async function expectJoined(page: Page) {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "joinDashPay" }));
  await expect(page.getByTestId("join-joined")).toContainText(`@${LABEL}`);
}

for (const theme of THEMES)
  test(`the welcome names the new username and offers three next steps (${theme})`, async ({ page }) => {
    const errors = await toWelcome(page, theme);
    await expect(screen(page).locator("h2")).toHaveText(`Welcome to DashPay, @${LABEL}`);
    await expect(screen(page).locator("h2")).toBeFocused();
    await expect(page.getByTestId("welcome-steps").getByRole("button")).toHaveText([
      /Add a photo and name/,
      /Find friends/,
      /Share my username/,
    ]);
    for (const name of ["Add a photo and name", "Find friends", "Share my username"])
      await expect(page.getByTestId("welcome-steps").getByRole("button", { name, exact: true })).toBeVisible();
    await expect(page.getByTestId("registration-wallet")).toHaveText("Go to wallet");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

test("with focus elsewhere when the run ends, the welcome is announced and focus stays", async ({ page }) => {
  const errors = await toWelcome(page, "light", () => page.getByTestId("sidebar-home").focus());
  await expect(page.getByTestId("sidebar-home")).toBeFocused();
  await expect(page.getByTestId("registration-announced")).toHaveText(`Welcome to DashPay, @${LABEL}. Your username is ready. Friends can now find you and pay you by name.`);
  expect(errors).toEqual([]);
});

test("Add a photo and name opens Edit Profile on My Profile", async ({ page }) => {
  const errors = await toWelcome(page);
  await page.getByTestId("welcome-editProfile").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
  await expect(page.getByTestId("edit-profile-sheet")).toBeVisible();
  await page.keyboard.press("Escape");
  await expectJoined(page);
  expect(errors).toEqual([]);
});

test("Find friends opens Contacts with Add contact on Search", async ({ page }) => {
  const errors = await toWelcome(page);
  await page.getByTestId("welcome-findFriends").click();
  await expect(page.getByTestId("page-contacts")).toBeVisible();
  await expect(page.getByTestId("add-contact")).toBeVisible();
  await expect(page.getByTestId("add-contact-tabs-search")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("add-contact-search")).toBeVisible();
  await expectJoined(page);
  expect(errors).toEqual([]);
});

test("Share my username shows the DashPay QR and link over the welcome", async ({ page }) => {
  const errors = await toWelcome(page);
  await page.getByTestId("welcome-share").click();
  const sheet = page.getByTestId("welcome-share-sheet");
  await expect(sheet).toBeVisible();
  await expect(sheet.getByTestId("share-who").locator(".share-who-text")).toHaveText("@alice-77");
  await expect(sheet.getByTestId("add-contact-qr")).toBeVisible();
  await expect(sheet.getByTestId("add-contact-qr-link")).toContainText("dashpay://");
  await expect(sheet.getByTestId("add-contact-copy-link")).toBeVisible();
  await sheet.getByRole("button", { name: "Close" }).click();
  await expect(sheet).toHaveCount(0);
  await page.getByTestId("welcome-share").click();
  await expect(sheet).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  await expect(page.getByTestId("welcome-share")).toBeFocused();
  // WebKit on macOS does not focus a button it clicks: focus still comes back to it.
  await page.getByTestId("welcome-share").blur();
  await page.getByTestId("welcome-share").dispatchEvent("click");
  await expect(sheet).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  await expect(page.getByTestId("welcome-share")).toBeFocused();
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  expect(errors).toEqual([]);
});

test("the welcome's share sheet closes once the engine refuses the link, and waits for another click", async ({ page }) => {
  const errors = await toWelcome(page);
  const share = page.getByTestId("welcome-share");
  await share.click();
  const sheet = page.getByTestId("welcome-share-sheet");
  await expect(sheet.getByTestId("add-contact-qr")).toBeVisible();
  const updating = (on: boolean) =>
    page.evaluate((on) => {
      const f = window.__dwFixture!;
      f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, namesUpdating: on }));
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
    }, on);
  // The engine's names pass is out: it has no link to give, so the sheet closes and Share waits.
  await updating(true);
  await expect(sheet).toHaveCount(0);
  await expect(share).toBeDisabled();
  await expect(page.locator(".registration-title")).toBeFocused();
  // The link comes back: Share is offered again, and the sheet waits for a click.
  await updating(false);
  await expect(share).toBeEnabled();
  await expect(sheet).toHaveCount(0);
  await share.click();
  await expect(sheet.getByTestId("add-contact-qr")).toBeVisible();
  await expect(screen(page)).toHaveAttribute("data-kind", "done");
  expect(errors).toEqual([]);
});

test("Go to wallet leaves the welcome for Home; Join DashPay then says you joined", async ({ page }) => {
  const errors = await toWelcome(page);
  await page.getByTestId("registration-wallet").click();
  await expect(page.getByTestId("hero")).toBeVisible();
  await expectJoined(page);
  expect(errors).toEqual([]);
});
