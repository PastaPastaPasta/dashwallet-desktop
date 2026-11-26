import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { IDENTITY } from "../src/fixture/dashpay";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP1-10: My Profile, the username request card, the credits block and the top-up sheet on the
// fixture backend, by parity ID: IOS-068 (the username request status: tallies, deadline,
// temporary name), IOS-069 (the profile sheet: names, credits, top-up from Core; DASHPAY F8's
// IOS-131 presets); Android's AND-071 (voting) and AND-078 (top-up, low-credit warnings).

async function openProfile(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("page-profile")).toBeVisible();
  return errors;
}

const body = (page: Page) => page.getByTestId("profile-body");
const credits = (page: Page) => page.getByTestId("profile-credits");
const request = (page: Page) => page.getByTestId("profile-request");
const sheet = (page: Page) => page.getByTestId("topup-body");

/** Each page state the design inventories (P2–P6, C1–C7, R1–R6), its scenario and what shows. */
const STATES: Record<string, { scenario: string; extra?: string; ready: (page: Page) => Promise<void> }> = {
  P2: { scenario: "dashpay-registered", extra: "&fail=dashpay_status:none", ready: (p) => expect(body(p)).toHaveAttribute("data-state", "error") },
  P3: { scenario: "dashpay-no-identity", ready: (p) => expect(body(p)).toHaveAttribute("data-state", "noProfile") },
  "P3-registering": { scenario: "dashpay-registering", ready: (p) => expect(p.getByTestId("dashpay-card")).toHaveAttribute("data-state", "creating") },
  P4: { scenario: "dashpay-registered", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("@alice") },
  P5: { scenario: "dashpay-no-username", ready: (p) => expect(p.getByTestId("profile-choose-name")).toBeVisible() },
  "P5-updating": { scenario: "dashpay-names-updating", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("Updating…") },
  "P5-unknown": { scenario: "dashpay-names-unknown", ready: (p) => expect(p.getByTestId("profile-username")).toHaveText("Username not loaded") },
  C2: { scenario: "dashpay-credits-low", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "low") },
  C3: { scenario: "dashpay-low-credits", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "empty") },
  C4: { scenario: "dashpay-credits-unknown", ready: (p) => expect(credits(p)).toHaveAttribute("data-level", "unknown") },
  C5: { scenario: "dashpay-credits-stale", ready: (p) => expect(credits(p)).toContainText("Updating…") },
  C6: { scenario: "dashpay-top-up-pending", ready: (p) => expect(p.getByTestId("profile-pending-finish")).toBeVisible() },
  "C6-unknown": { scenario: "dashpay-top-up-unknown", ready: (p) => expect(p.getByTestId("profile-pending-arrived")).toBeVisible() },
  C7: { scenario: "dashpay-top-up-stuck", ready: (p) => expect(p.getByTestId("profile-stuck")).toBeVisible() },
  R1: { scenario: "dashpay-contest-pending", ready: (p) => expect(p.getByTestId("profile-tally")).toBeVisible() },
  R2: { scenario: "dashpay-contest-pending-no-end", ready: (p) => expect(request(p)).toContainText("We will notify you when voting ends.") },
  "R1-instant": { scenario: "dashpay-contest-no-temporary", ready: (p) => expect(p.getByTestId("profile-instant")).toBeVisible() },
  R3: { scenario: "dashpay-contest-won", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "won") },
  R4: { scenario: "dashpay-contest-lost", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "lost") },
  R5: { scenario: "dashpay-contest-locked", ready: (p) => expect(request(p)).toHaveAttribute("data-state", "locked") },
  R6: { scenario: "dashpay-contest-pending", extra: "&fail=contest_status:none", ready: (p) => expect(p.getByTestId("profile-request-failed")).toBeVisible() },
};

for (const theme of THEMES) {
  for (const [state, { scenario, extra, ready }] of Object.entries(STATES)) {
    test(`IOS-068 / IOS-069 My Profile ${state} ${theme}: renders, 0 serious axe issues`, async ({ page }) => {
      const errors = await openProfile(page, scenario, theme, extra);
      await ready(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test(`IOS-069 / AND-078 top-up sheet ${theme}: amount, confirm, outcome; 0 serious axe issues`, async ({ page }) => {
    const errors = await openProfile(page, "dashpay-registered", theme);
    await page.getByTestId("profile-top-up").click();
    await expect(sheet(page)).toHaveAttribute("data-step", "amount");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-step", "done");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

// PRODUCT #12: Share username opens the welcome's sheet, My QR with the dashpay://user link.
test("Share username shows a large DashPay QR and link; Copy Link copies it; Escape returns focus", async ({ page }) => {
  // The clipboard as a variable: headless engines refuse writes (tools.spec.ts).
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) } });
  });
  const errors = await openProfile(page, "dashpay-registered");
  const share = page.getByTestId("profile-share");
  await expect(share).toHaveText("Share Username");
  await share.click();
  const shareSheet = page.getByTestId("profile-share-sheet");
  await expect(shareSheet).toBeVisible();
  await expect(shareSheet.getByRole("heading", { name: "Share my username" })).toBeVisible();
  // Whose code it is, for the friend about to scan it.
  await expect(shareSheet.getByTestId("share-who").locator(".share-who-text")).toHaveText("Alice Liddell@alice");
  const link = `dashpay://user?id=${IDENTITY.alice}&username=alice`;
  await expect(shareSheet.getByTestId("add-contact-qr-link")).toHaveText(link);
  const qr = shareSheet.getByTestId("add-contact-qr");
  await expect(qr).toBeVisible();
  expect((await qr.boundingBox())!.width).toBeGreaterThanOrEqual(240);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await shareSheet.getByTestId("add-contact-copy-link").click();
  expect(await page.evaluate(() => (window as unknown as { __copied?: string }).__copied)).toBe(link);
  await page.keyboard.press("Escape");
  await expect(shareSheet).toHaveCount(0);
  await expect(share).toBeFocused();
  await share.click();
  await shareSheet.getByRole("button", { name: "Close" }).click();
  await expect(shareSheet).toHaveCount(0);
  await expect(share).toBeFocused();
  // WebKit on macOS does not focus a button it clicks: focus still comes back to it.
  await share.blur();
  await share.dispatchEvent("click");
  await expect(shareSheet).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(shareSheet).toHaveCount(0);
  await expect(share).toBeFocused();
  expect(errors).toEqual([]);
});

test("a username being refreshed has no link yet: Share username hides, and an open sheet closes", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-registered");
  const share = page.getByTestId("profile-share");
  const shareSheet = page.getByTestId("profile-share-sheet");
  const names = (updating: boolean) =>
    page.evaluate((updating) => {
      const f = window.__dwFixture!;
      f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, namesUpdating: updating }));
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
    }, updating);
  await share.click();
  await expect(shareSheet.getByTestId("add-contact-qr")).toBeVisible();
  await names(true);
  await expect(shareSheet).toHaveCount(0);
  await expect(share).toHaveCount(0);
  // The button that opened the sheet is gone: focus lands on Edit Profile beside it.
  await expect(page.getByTestId("profile-edit")).toBeFocused();
  await expect(page.getByTestId("profile-username")).toHaveText("@alice");
  await names(false);
  await expect(share).toBeVisible();
  // The sheet waits for another click rather than reopening by itself.
  await expect(shareSheet).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("Share username follows the engine's link, not the identity list: hidden while the link is refused", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-registered");
  const share = page.getByTestId("profile-share");
  await expect(share).toBeVisible();
  const emitNames = () =>
    page.evaluate(() => {
      const f = window.__dwFixture!;
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
    });
  // The identity list still says the name is settled, but the engine refuses a link for it: its
  // names pass is out, and the list's own re-read has not come back yet.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    (window as unknown as { __release: () => void }).__release = f.hold("identities");
    f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, namesUpdating: true }));
  });
  await emitNames();
  await expect(share).toHaveCount(0);
  await expect(page.getByTestId("profile-username")).toHaveText("@alice");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, namesUpdating: false }));
    (window as unknown as { __release: () => void }).__release();
  });
  await emitNames();
  await expect(share).toBeVisible();
  // A re-read that fails for another reason keeps the button: the cached link is still right.
  const reads = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "dashpay_my_user_link").length);
  const before = await reads();
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_my_user_link", "rpc"));
  await emitNames();
  await expect.poll(reads).toBeGreaterThan(before);
  await expect(share).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.fail("dashpay_my_user_link", null));
  // A slow re-read of the same name keeps the button too.
  await page.evaluate(() => {
    (window as unknown as { __release: () => void }).__release = window.__dwFixture!.hold("dashpay_my_user_link");
  });
  await emitNames();
  await expect(share).toBeVisible();
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(share).toBeVisible();
  expect(errors).toEqual([]);
});

test("an open sheet drops its code once the engine refuses the link, though the identity list is not re-read yet", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-share").click();
  const shareSheet = page.getByTestId("profile-share-sheet");
  await expect(shareSheet.getByTestId("add-contact-qr")).toBeVisible();
  // The names pass holds the username: the engine refuses the link, and the identity list's
  // re-read is still out, so the page's snapshot still says the name is settled.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    (window as unknown as { __release: () => void }).__release = f.hold("identities");
    f.world.dashpay.identities = f.world.dashpay.identities.map((i) => ({ ...i, namesUpdating: true }));
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
  });
  await expect(shareSheet.getByTestId("add-contact-qr-updating")).toBeVisible();
  await expect(shareSheet.getByTestId("add-contact-qr")).toHaveCount(0);
  await expect(page.getByTestId("profile-share")).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(shareSheet).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("an open sheet drops its code once the engine says the username is gone", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-share").click();
  const shareSheet = page.getByTestId("profile-share-sheet");
  await expect(shareSheet.getByTestId("add-contact-qr")).toBeVisible();
  // The engine has no username for the identity any more; the identity list's re-read is still out.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    (window as unknown as { __release: () => void }).__release = f.hold("identities");
    f.world.dashpay.identities = f.world.dashpay.identities.map((i, n) => (n === 0 ? { ...i, names: [], mainName: null } : i));
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
  });
  await expect(shareSheet.getByTestId("add-contact-qr-no-username")).toBeVisible();
  await expect(shareSheet.getByTestId("add-contact-qr")).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(shareSheet).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a new main name while the sheet is open shows no link for the old one", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-share").click();
  const shareSheet = page.getByTestId("profile-share-sheet");
  const linkText = shareSheet.getByTestId("add-contact-qr-link");
  await expect(linkText).toHaveText(`dashpay://user?id=${IDENTITY.alice}&username=alice`);
  // Alice makes another name she owns her main one; its link is read again, slowly.
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    (window as unknown as { __release: () => void }).__release = f.hold("dashpay_my_user_link");
    f.world.dashpay.identities = f.world.dashpay.identities.map((i, n) => (n === 0 ? { ...i, names: [...i.names, "alice2"], mainName: "alice2" } : i));
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
  });
  await expect(shareSheet.getByTestId("share-who").locator(".share-who-username")).toHaveText("@alice2");
  await expect(linkText).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(linkText).toHaveText(`dashpay://user?id=${IDENTITY.alice}&username=alice2`);
  expect(errors).toEqual([]);
});

test("without a username there is no Share username: Choose a username instead", async ({ page }) => {
  await openProfile(page, "dashpay-no-username");
  await expect(page.getByTestId("profile-choose-name")).toBeVisible();
  await expect(page.getByTestId("profile-share")).toHaveCount(0);
  await openProfile(page, "dashpay-no-identity");
  await expect(body(page)).toHaveAttribute("data-state", "noProfile");
  await expect(page.getByTestId("profile-share")).toHaveCount(0);
});

test("IOS-069 P4: header, usernames, identity id with copy, credits and the contest outcome rows", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await expect(page.getByTestId("profile-name")).toHaveText("Alice Liddell");
  await expect(page.getByTestId("profile-header")).toContainText("Down the rabbit hole");
  await expect(page.getByTestId("profile-edit")).toBeEnabled();
  await expect(page.getByTestId("profile-usernames")).toContainText("@alice");
  await expect(page.getByTestId("profile-identity").getByRole("button", { name: "Copy identity ID" })).toBeVisible();
  await expect(page.getByTestId("profile-credits-figure")).toHaveText(/^1\.845/);
  await expect(page.getByTestId("profile-credits-estimate")).toHaveText("≈ 1,961 contact requests · 1,537 profile updates");
  // The fixture's journal holds a lost contest for "al" (D12).
  await expect(page.getByTestId("profile-outcomes")).toContainText("@al — went to another user");
});

test("DP4-02: the header shows the verified avatar; a failed read shows the initials", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-avatar");
  const avatar = page.getByTestId("profile-header").locator(".contact-avatar");
  await expect(avatar.locator("img")).toHaveAttribute("src", /^data:image\/png;base64,iVBOR/);
  await expect(avatar).toHaveAttribute("data-picture", "");
  await openProfile(page, "dashpay-avatar", "light", "&fail=dashpay_avatar:none");
  await expect(page.getByTestId("profile-name")).toHaveText("Alice Liddell");
  await expect(avatar.locator("img")).toHaveCount(0);
  await expect(avatar).toHaveText(/\S/);
  expect(errors).toEqual([]);
});

test("C8 / D7: discreet mode masks the credit figure and the estimate", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("toolbar-discreet").click();
  await expect(page.getByTestId("profile-credits-figure")).not.toContainText("1.845");
  await expect(page.getByTestId("profile-credits-estimate")).not.toContainText("1,961");
});

test("IOS-069 D5: a custom amount below the floor is refused before Continue; a preset tops up and the balance moves", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-other").click();
  await page.getByTestId("topup-other-field").fill("0.001");
  await expect(page.getByTestId("topup-amount-problem")).toContainText("The smallest top-up is 0.01");
  await expect(page.getByTestId("topup-next")).toBeDisabled();
  await page.getByTestId("topup-preset-10000000").click();
  await page.getByTestId("topup-next").click();
  await expect(page.getByTestId("topup-quote")).toContainText("Top-up fee");
  await expect(sheet(page)).toContainText("You have chosen to top up your identity.");
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
  await page.getByTestId("topup-done").click();
  await expect(page.getByTestId("topup-sheet")).toHaveCount(0);
  await expect(page.getByTestId("profile-credits-figure")).toHaveText(/^1\.94/);
});

test("IOS-069 D15: a pending top-up holds Top up; Finish applies it", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-pending");
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
  await expect(page.getByTestId("profile-top-up-reason")).toHaveText("Finish the pending top-up first.");
  await expect(page.getByTestId("profile-pending-arrived")).toHaveCount(0);
  await page.getByTestId("profile-pending-finish").click();
  await authorize(page);
  await expect(page.getByTestId("profile-pending-finish")).toHaveCount(0);
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("IOS-069 D15: Credits arrived resolves a proved lock whose consumption is unknown", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-unknown");
  await page.getByTestId("profile-pending-arrived").click();
  await expect(page.getByTestId("profile-pending-arrived")).toHaveCount(0);
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("D16: a stuck top-up funding routes to Tools ▸ Repair, which resolves it", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-stuck");
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
  await page.getByTestId("profile-stuck-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
  const row = page.getByTestId("repair-stuck-topup/0-1/funding");
  await expect(row).toContainText("A top-up of @alice's credits");
  await page.getByTestId("repair-resolve-topup/0-1/funding").click();
  await expect(page.getByTestId("repair-confirm")).toContainText("Whether the top-up's payment reached the network");
  await page.getByTestId("repair-confirm-ok").click();
  await authorize(page);
  await expect(page.getByTestId("repair-outcome")).toContainText("You can top up again once it is confirmed.");
  await expect(row).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-top-up")).toBeEnabled();
});

test("T8 / D17: the quote's refusal shows before Confirm; a refusal at pay routes by its code", async ({ page }) => {
  await openProfile(page, "dashpay-registered", "light", "&fail=credits_top_up_quote:none");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await expect(page.getByTestId("topup-quote-error")).toBeVisible();
  await expect(page.getByTestId("topup-pay")).toBeDisabled();
});

test("T8 / D16 / D22: a funding left unknown at pay routes to Repair, which lists its step", async ({ page }) => {
  await openProfile(page, "dashpay-top-up-lost");
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(sheet(page)).toHaveAttribute("data-step", "refused");
  await expect(page.getByTestId("topup-refused-back")).toHaveCount(0);
  await page.getByTestId("topup-repair").click();
  await expect(page.getByTestId("tools-repair")).toBeVisible();
  await expect(page.getByTestId("repair-stuck-topup/0-1/funding")).toContainText("A top-up of @alice's credits");
  // No Platform signal followed the refusal; the page read the stuck funding again anyway.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-stuck")).toBeVisible();
  await expect(page.getByTestId("profile-top-up")).toBeDisabled();
});

test("D14: a top-up that ends while the sheet is closed is offered on the page and shown once", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("credits_top_up"));
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-next").click();
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(sheet(page)).toHaveAttribute("data-step", "running");
  await page.getByTestId("topup-sheet").getByRole("button", { name: "Close" }).click();
  await expect(page.getByTestId("profile-top-up-reason")).toHaveText("Topping up…");
  await release.evaluate((r) => r());
  await expect(page.getByTestId("profile-top-up-unseen")).toContainText("Your top-up has an update.");
  await page.getByTestId("profile-top-up-show").click();
  await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
  await page.getByTestId("topup-done").click();
  await expect(page.getByTestId("profile-top-up-unseen")).toHaveCount(0);
  await page.getByTestId("profile-top-up").click();
  await expect(sheet(page)).toHaveAttribute("data-step", "amount");
});

test("IOS-068 R1 / D11: the tally, the deadline and the temporary name; voting info opens", async ({ page }) => {
  await openProfile(page, "dashpay-contest-pending");
  await expect(body(page)).toHaveAttribute("data-state", "contest");
  await expect(request(page)).toHaveAttribute("data-state", "open");
  await expect(request(page)).toContainText("Voting – @alice");
  await expect(request(page)).toContainText("Until then you are @alice007.");
  await expect(page.getByTestId("profile-tally")).toContainText("Votes for you12");
  await expect(page.getByTestId("profile-instant")).toHaveCount(0);
  await page.getByTestId("profile-voting-info").click();
  await expect(page.getByTestId("voting-info")).toBeVisible();
});

test("D13: Create instant username opens the credits path and refuses a contested label", async ({ page }) => {
  await openProfile(page, "dashpay-contest-no-temporary");
  await expect(page.getByTestId("profile-username")).toHaveText("@alice · Pending");
  await page.getByTestId("profile-instant").click();
  await expect(page.getByTestId("registration-name")).toContainText("Create instant username");
  await page.getByTestId("registration-name-field").fill("bobby");
  await expect(page.getByTestId("registration-instant-only")).toBeVisible();
  await expect(page.getByTestId("registration-next")).toBeDisabled();
  await page.getByTestId("registration-name-field").fill("bobby2");
  await expect(page.getByTestId("registration-instant-only")).toHaveCount(0);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
});

test("R6: a failed contest read keeps the card and Try again reads it", async ({ page }) => {
  await openProfile(page, "dashpay-contest-pending", "light", "&fail=contest_status:none");
  await expect(request(page)).toContainText("Voting – @alice");
  await expect(page.getByTestId("profile-request-failed")).toContainText("Could not read the votes.");
  await page.evaluate(() => window.__dwFixture!.fail("contest_status", null));
  await page.getByTestId("profile-request-retry").click();
  await expect(page.getByTestId("profile-request-failed")).toHaveCount(0);
  await expect(page.getByTestId("profile-tally")).toContainText("Votes for you12");
});

test("D21: the contested chip opens My Profile", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-contest-pending", "light"));
  await page.getByTestId("toolbar-chip").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
});

test("S19 / D8: Home's low-credit card buys credits: My Profile with the top-up sheet open", async ({ page }) => {
  await page.goto(fixtureUrl("dashpay-credits-low", "light"));
  const card = page.getByTestId("dashpay-card");
  await expect(card).toHaveAttribute("data-state", "lowCredits");
  await card.getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
  await expect(sheet(page)).toHaveAttribute("data-step", "amount");
});

test("D20: Settings ▸ General's DashPay card opens My Profile, or Join without an identity", async ({ page }) => {
  for (const [scenario, label, route] of [
    ["dashpay-registered", "My Profile", "profile"],
    ["dashpay-no-identity", "Join DashPay", "joinDashPay"],
  ] as const) {
    await page.goto(fixtureUrl(scenario, "light"));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    const open = page.getByTestId("settings-dashpay-open");
    await expect(open).toHaveText(label);
    await open.click();
    await expect(page.getByTestId(`page-${route}`)).toBeVisible();
  }
});

test("D2: My Profile without an identity is DP1-08's card with its action", async ({ page }) => {
  await openProfile(page, "dashpay-no-identity");
  await expect(page.getByTestId("profile-body")).toContainText("You have no DashPay profile yet");
  await page.getByTestId("dashpay-card-action").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
});

// DP1-09 D4 reversed: the request's details while the own contest is open, and the won card.
test("the request card: a temporary name that failed offers Try again prefilled; short credits offer Top up", async ({ page }) => {
  await openProfile(page, "dashpay-instant-failed");
  await expect(request(page)).toContainText("The instant username “alice2” could not be registered");
  await expect(page.getByTestId("profile-instant")).toHaveCount(0);
  await page.getByTestId("profile-instant-retry").click();
  await expect(page.getByTestId("registration-name-field")).toHaveValue("alice2");

  await openProfile(page, "dashpay-contest-instant-short");
  await expect(request(page)).toContainText("Not enough credits for an instant username.");
  await page.getByTestId("profile-instant-top-up").click();
  await expect(sheet(page)).toBeVisible();
});

test("vote won: the contested name is now the main one, the instant one kept; no prompt", async ({ page }) => {
  await openProfile(page, "dashpay-instant-won");
  await expect(request(page)).toHaveAttribute("data-state", "won");
  await expect(request(page)).toContainText("“alice” is now your main username. Your instant username “alice2” stays yours too.");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openProfile(page, "dashpay-instant-lost");
  await expect(request(page)).toHaveAttribute("data-state", "lost");
  await expect(request(page)).not.toContainText("main username");
});

test("the success line on the request card", async ({ page }) => {
  await openProfile(page, "dashpay-instant-ok");
  await expect(request(page)).toContainText("In the meantime you are reachable at “alice2”");
});
