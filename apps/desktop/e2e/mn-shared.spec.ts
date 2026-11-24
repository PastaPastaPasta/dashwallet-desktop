import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { abandonFee, SH_TEXT } from "../src/fixture/shared";

/** `sh-close-out`'s listed Abandon charge: its one reserved coin spent whole. */
const CHARGE = abandonFee(1);
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// Shared masternodes (MG-10b S5, S5b) on the fixture: Start or join, the coordinator's setup to its
// invitation, the session sheet's stages, a pasted invitation joined, Close offering Discard only
// while the engine reads the session as discardable, and Sign, Send and Send again with their
// confirm, refusal and mainnet states. DW_SHOTS=<dir> also writes each state's PNG.

const SHOTS = process.env.DW_SHOTS;

const open = async (page: Page, scenario: string, theme: Theme, extra = "") => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
};

const openSession = async (page: Page, scenario: string, theme: Theme) => {
  const errors = await open(page, scenario, theme);
  await page.getByTestId("mn-shared").click();
  await page.getByTestId("sh-in-progress").getByRole("button").first().click();
  await expect(page.getByTestId("sh-session")).toBeVisible();
  return errors;
};

/** Fills the passphrase when the confirm asks for one, then confirms. */
const confirmWith = async (page: Page, action: string) => {
  const field = page.getByTestId(`sh-${action}-credential-passphrase`);
  if (await field.isVisible()) await field.fill(FIXTURE_PASSPHRASE);
  await page.getByTestId(`sh-${action}-go`).click();
};

const shot = async (page: Page, name: string, theme: Theme) => {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}-${theme}.png` });
};

for (const theme of THEMES) {
  test(`shared masternode ${theme}: setup to invitation, then the session sheet`, async ({ page }, info) => {
    const errors = await open(page, "sh-capability-active", theme);
    await page.getByTestId("mn-shared").click();
    await expect(page.getByTestId("sh-start")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-start", theme);
    await page.getByTestId("sh-start-button").click();
    await expect(page.getByTestId("sh-setup-step-shares")).toBeVisible();
    await page.getByTestId("sh-setup-add").click();
    await page.getByTestId("sh-setup-label-1").fill("Bob");
    await page.getByTestId("sh-setup-continue").click();
    await expect(page.getByTestId("sh-setup-problems")).toContainText("Give every participant a name.");
    await page.getByTestId("sh-setup-label-2").fill("Carol");
    await page.getByTestId("sh-setup-amount-0").fill("400");
    await page.getByTestId("sh-setup-amount-1").fill("300");
    await page.getByTestId("sh-setup-amount-2").fill("300");
    await expect(page.getByTestId("sh-setup-total")).toContainText("3 shares");
    if (info.project.name === "chromium") await shot(page, "sh-setup-shares", theme);
    await page.getByTestId("sh-setup-continue").click();
    await page.getByTestId("sh-setup-service").fill("203.0.113.30:19999");
    await page.getByTestId("sh-setup-operator-self").check();
    await page.getByTestId("sh-setup-operator-share").fill("5");
    if (info.project.name === "chromium") await shot(page, "sh-setup-masternode", theme);
    await page.getByTestId("sh-setup-continue").click();
    await expect(page.getByTestId("sh-setup-step-exit")).toBeVisible();
    await page.getByTestId("sh-setup-penalty").fill("20");
    if (info.project.name === "chromium") await shot(page, "sh-setup-exit", theme);
    await page.getByTestId("sh-setup-continue").click();
    await expect(page.getByTestId("sh-setup-estimate")).toBeVisible();
    await expect(page.getByTestId("sh-reservation-note")).toContainText("every coin chosen for it stays unavailable for other payments");
    if (info.project.name === "chromium") await shot(page, "sh-setup-coins", theme);
    await page.getByTestId("sh-setup-continue").click();
    await expect(page.getByTestId("sh-setup-step-invite")).toBeVisible();
    await expect(page.getByTestId("sh-setup-check-code")).not.toBeEmpty();
    if (info.project.name === "chromium") await shot(page, "sh-setup-invite", theme);
    await page.getByTestId("sh-setup-continue").click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-stages")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-session-draft", theme);
    expect(errors).toEqual([]);
  });

  for (const [how, delay] of [["click", ""], ["Enter", ""], ["copy, then click", ""], ["copy, then click", "&delay=400"], ["twice", ""], ["twice", "&delay=100"], ["twice", "&delay=400"]] as const) {
    test(`shared masternode ${theme}: Done (${how}${delay ? `, engine ${delay.split("=")[1]} ms` : ""}) after Create invitation opens the session, never Discard`, async ({ page }) => {
      test.setTimeout(60_000);
      const errors = await open(page, "sh-capability-active", theme, delay);
      await page.getByTestId("mn-shared").click();
      await page.getByTestId("sh-start-button").click();
      await page.getByTestId("sh-setup-label-1").fill("Bob");
      await page.getByTestId("sh-setup-amount-0").fill("600");
      await page.getByTestId("sh-setup-amount-1").fill("400");
      await page.getByTestId("sh-setup-continue").click();
      await page.getByTestId("sh-setup-service").fill("203.0.113.30:19999");
      await page.getByTestId("sh-setup-operator-self").check();
      await page.getByTestId("sh-setup-operator-share").fill("5");
      await page.getByTestId("sh-setup-continue").click();
      await expect(page.getByTestId("sh-setup-step-exit")).toBeVisible();
      await page.getByTestId("sh-setup-continue").click();
      await expect(page.getByTestId("sh-setup-estimate")).toBeVisible();
      await page.getByTestId("sh-setup-continue").click();
      await expect(page.getByTestId("sh-setup-step-invite")).toBeVisible();
      const done = page.getByTestId("sh-setup-continue");
      if (how === "click") await done.click();
      else if (how === "copy, then click") {
        // The regtest rehearsal: Copy invitation, then one mouse click on Done.
        await page.getByTestId("sh-setup-copy").click();
        const box = (await done.boundingBox())!;
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      }
      else if (how === "twice") {
        // A second click where Done was, as a quick double click lands it.
        const box = (await done.boundingBox())!;
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await page.waitForTimeout(250);
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      }
      else {
        await done.focus();
        await page.keyboard.press(how, { delay: 100 });
      }
      await expect(page.getByTestId("sh-session")).toBeVisible();
      await expect(page.getByTestId("sh-stages")).toBeVisible();
      await page.waitForTimeout(1500);
      await expect(page.getByTestId("sh-close")).toHaveCount(0);
      await expect(page.getByTestId("sh-session")).toBeVisible();
      expect(errors).toEqual([]);
    });
  }

  test(`shared masternode ${theme}: a pasted invitation is joined; Close offers Discard`, async ({ page }, info) => {
    const errors = await open(page, "sh-capability-active", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-paste").fill(SH_TEXT.invitation);
    await page.getByTestId("sh-read").click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-join-button")).toBeVisible();
    await expect(page.getByTestId("sh-reservation-note")).toContainText("every coin chosen for it stays unavailable for other payments");
    if (info.project.name === "chromium") await shot(page, "sh-join", theme);
    await page.getByTestId("sh-join-button").click();
    await expect(page.getByTestId("sh-copy")).toBeVisible();
    await expect(page.getByTestId("sh-reservation-note")).toHaveCount(0);
    await page.getByTestId("sh-close-button").click();
    await expect(page.getByTestId("sh-close")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-close", theme);
    await page.getByTestId("sh-close-discard").click();
    await expect(page.getByTestId("sh-close")).toBeHidden();
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: locked terms to approve, and a signed session that may be out has no Discard`, async ({ page }, info) => {
    await open(page, "sh-participant-approve", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-approve")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-approve", theme);
    await page.goto(fixtureUrl("sh-participant-combined", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("sidebar-masternodes").click();
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-row-signLater").or(page.getByTestId("sh-row-sign"))).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-sign-later", theme);
    await page.goto(fixtureUrl("sh-close-out", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.getByTestId("sidebar-masternodes").click();
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-row-sendSigned")).toBeVisible();
    await expect(page.getByTestId("sh-session")).not.toContainText("nothing has left this wallet");
    await page.getByTestId("sh-close-button").click();
    await expect(page.getByTestId("sh-close")).toHaveCount(0);
    await expect(page.getByTestId("sh-session")).toBeHidden();
  });

  test(`shared masternode ${theme}: with two shares open, Join waits until one is picked`, async ({ page }, info) => {
    const errors = await open(page, "sh-participant-invited-two", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-join-pick")).toBeVisible();
    await expect(page.getByTestId("sh-join-button")).toBeDisabled();
    await page.getByTestId("sh-join-pick-2").click();
    await expect(page.getByTestId("sh-join-button")).toBeEnabled();
    if (info.project.name === "chromium") await shot(page, "sh-join-pick", theme);
    await page.getByTestId("sh-join-button").click();
    await expect(page.getByTestId("sh-copy")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: a session that can't be read is listed, with Discard only while nothing may be out`, async ({ page }, info) => {
    const errors = await open(page, "sh-unreadable", theme);
    await page.getByTestId("mn-shared").click();
    const free = page.getByTestId("sh-unreadable-7C7C7C");
    const out = page.getByTestId("sh-unreadable-9E9E9E");
    await expect(free).toContainText("Session 7C7C7C can't be read");
    await expect(free).toContainText("Discard it to release the coins it reserved.");
    await expect(out).toContainText("the others come back about 15 minutes later");
    await expect(page.getByTestId("sh-unreadable-discard-9E9E9E")).toHaveCount(0);
    await expect(page.getByTestId("sh-unreadable-5A5A5A"), "released: nothing left to show").toHaveCount(0);
    if (info.project.name === "chromium") await shot(page, "sh-unreadable", theme);
    await page.getByTestId("sh-unreadable-discard-7C7C7C").click();
    await expect(page.getByTestId("sh-unreadable-confirm")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-unreadable-confirm", theme);
    await page.getByTestId("sh-unreadable-confirm-discard").click();
    await expect(page.getByTestId("sh-unreadable-confirm")).toBeHidden();
    await expect(free).toHaveCount(0);
    await expect(out).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: a signed-out session names Abandon, which spends its coins back for exactly the listed charge`, async ({ page }, info) => {
    const errors = await open(page, "sh-close-out", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    // Where sends are on, the signed-out participant's row is the hand-off; Abandon stands beside it.
    await expect(page.getByTestId("sh-row-sendSigned")).toBeVisible();
    await expect(page.getByTestId("sh-abandon-button")).toHaveText("Abandon");
    await page.getByTestId("sh-abandon-button").click();
    await expect(page.getByTestId("sh-abandon")).toBeVisible();
    await expect(page.getByTestId("sh-abandon-text")).toContainText(`paying a ${(CHARGE / 1e8).toFixed(8)} tDASH network fee`);
    await page.getByTestId("sh-abandon-passphrase").fill(FIXTURE_PASSPHRASE);
    await expect(page.getByTestId("sh-abandon-abandon"), "not until ticked").toBeDisabled();
    await page.getByTestId("sh-abandon-understand").check();
    if (info.project.name === "chromium") await shot(page, "sh-abandon-confirm", theme);
    await page.getByTestId("sh-abandon-abandon").click();
    await expect(page.getByTestId("sh-abandon")).toBeHidden();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-abandon-button")).toHaveCount(0);
    await expect(page.getByTestId("sh-session")).not.toContainText("choose Abandon");
    if (info.project.name === "chromium") await shot(page, "sh-signed-out-abandoned", theme);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: Abandon is offered and runs on mainnet`, async ({ page }, info) => {
    const errors = await open(page, "sh-close-out", theme, "&network=mainnet");
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-row-signedOut")).toContainText("To cancel it, choose Abandon");
    if (info.project.name === "chromium") await shot(page, "sh-signed-out-abandon", theme);
    await page.getByTestId("sh-abandon-button").click();
    await expect(page.getByTestId("sh-abandon-text")).toContainText(`paying a ${(CHARGE / 1e8).toFixed(8)} DASH network fee`);
    await page.getByTestId("sh-abandon-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("sh-abandon-understand").check();
    if (info.project.name === "chromium") await shot(page, "sh-abandon-confirm-mainnet", theme);
    await page.getByTestId("sh-abandon-abandon").click();
    await expect(page.getByTestId("sh-abandon")).toBeHidden();
    await expect(page.getByTestId("sh-abandon-button")).toHaveCount(0);
    await page.goto(fixtureUrl("sh-unreadable", theme, "&network=mainnet"));
    await page.getByTestId("sidebar-masternodes").click();
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-unreadable-abandon-9E9E9E").click();
    await page.getByTestId("sh-unreadable-abandon-confirm-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("sh-unreadable-abandon-confirm-understand").check();
    await page.getByTestId("sh-unreadable-abandon-confirm-abandon").click();
    await expect(page.getByTestId("sh-unreadable-abandon-9E9E9E")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: a signed-out session with no coin left to spend neither offers nor names Abandon`, async ({ page }) => {
    const errors = await open(page, "sh-signed-out-abandoned", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-abandon-button")).toHaveCount(0);
    await expect(page.getByTestId("sh-session")).not.toContainText("Abandon");
    await expect(page.getByTestId("sh-close-discard")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: an unreadable session that may be out is abandoned`, async ({ page }, info) => {
    const errors = await open(page, "sh-unreadable", theme);
    await page.getByTestId("mn-shared").click();
    await expect(page.getByTestId("sh-unreadable-abandon-7C7C7C")).toHaveCount(0);
    await expect(page.getByTestId("sh-unreadable-9E9E9E")).toContainText("may already be on the network");
    await page.getByTestId("sh-unreadable-abandon-9E9E9E").scrollIntoViewIfNeeded();
    if (info.project.name === "chromium") await shot(page, "sh-unreadable-abandon", theme);
    await page.getByTestId("sh-unreadable-abandon-9E9E9E").click();
    await expect(page.getByTestId("sh-unreadable-abandon-confirm")).toBeVisible();
    await page.getByTestId("sh-unreadable-abandon-confirm-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("sh-unreadable-abandon-confirm-understand").check();
    if (info.project.name === "chromium") await shot(page, "sh-unreadable-abandon-confirm", theme);
    await page.getByTestId("sh-unreadable-abandon-confirm-abandon").click();
    await expect(page.getByTestId("sh-unreadable-abandon-confirm")).toBeHidden();
    await expect(page.getByTestId("sh-unreadable-9E9E9E")).toBeVisible();
    await expect(page.getByTestId("sh-unreadable-abandon-9E9E9E")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: a participant signs its coins, and the stage advances`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-participant-combined", theme);
    await expect(page.getByTestId("sh-row-sign")).toContainText("leaves this wallet when Alice sends it");
    await page.getByTestId("sh-sign").click();
    await expect(page.getByTestId("sh-sign-confirm")).toBeVisible();
    await expect(page.getByTestId("sh-sign-text")).toContainText("300");
    if (info.project.name === "chromium") await shot(page, "sh-sign-confirm", theme);
    await confirmWith(page, "sign");
    await expect(page.getByTestId("sh-sign-confirm")).toBeHidden();
    await expect(page.getByTestId("sh-row-sendSigned")).toBeVisible();
    await expect(page.getByTestId("sh-copy")).toBeVisible();
    await expect(page.getByTestId("sh-sign")).toHaveCount(0);
    if (info.project.name === "chromium") await shot(page, "sh-after-sign", theme);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: the coordinator sends the registration, which reads sent`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-coordinator-send", theme);
    await expect(page.getByTestId("sh-row-send")).toContainText("Everyone signed.");
    await page.getByTestId("sh-send").click();
    await expect(page.getByTestId("sh-send-confirm")).toBeVisible();
    await expect(page.getByTestId("sh-send-text")).toContainText("400.001");
    if (info.project.name === "chromium") await shot(page, "sh-send-confirm", theme);
    await confirmWith(page, "send");
    await expect(page.getByTestId("sh-send-confirm")).toBeHidden();
    await expect(page.getByTestId("sh-row-sent")).toContainText("waiting for 1 confirmation");
    await expect(page.getByTestId("sh-resend")).toBeVisible();
    await expect(page.getByTestId("sh-handoff")).toHaveCount(0);
    if (info.project.name === "chromium") await shot(page, "sh-after-send", theme);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: mainnet offers no Sign or Send and says why`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-participant-combined-mainnet", theme);
    await expect(page.getByTestId("sh-mainnet-note")).toBeVisible();
    await expect(page.getByTestId("sh-sign")).toHaveCount(0);
    await expect(page.getByTestId("sh-row-signLater")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-mainnet-sign", theme);
    await openSession(page, "sh-coordinator-send-mainnet", theme);
    await expect(page.getByTestId("sh-mainnet-note")).toBeVisible();
    await expect(page.getByTestId("sh-send")).toHaveCount(0);
    if (info.project.name === "chromium") await shot(page, "sh-mainnet-send", theme);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: Send again on an abandoned registration is refused with its reason`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-coordinator-sent-abandoned", theme);
    await page.getByTestId("sh-resend").click();
    await expect(page.getByTestId("sh-resend-text")).toContainText("Send the same transaction again");
    if (info.project.name === "chromium") await shot(page, "sh-resend-confirm", theme);
    await confirmWith(page, "resend");
    await expect(page.getByTestId("sh-resend-error")).toContainText("abandoned, so it can't be sent again");
    if (info.project.name === "chromium") await shot(page, "sh-refusal", theme);
    await openSession(page, "sh-coordinator-sent-unknown", theme);
    await expect(page.getByTestId("sh-handoff")).toContainText("It may still confirm.");
    if (info.project.name === "chromium") await shot(page, "sh-handoff-unknown", theme);
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: a sent registration's way out opens it in Activity`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-coordinator-sent", theme);
    await expect(page.getByTestId("sh-way-out-text")).toHaveCount(0);
    await page.getByTestId("sh-way-out-toggle").click();
    await expect(page.getByTestId("sh-way-out-text")).toContainText("first abandon the registration transaction in Activity");
    if (info.project.name === "chromium") await shot(page, "sh-way-out", theme);
    await page.getByTestId("sh-way-out-activity").click();
    await expect(page.getByTestId("sh-session")).toBeHidden();
    const chip = page.getByTestId("tx-route-filter");
    await expect(chip).toContainText("Session 3F9A2C");
    await expect(chip).toHaveAttribute("data-txids", "7e".repeat(32));
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: an overdue registration's way out opens expanded, and still collapses`, async ({ page }) => {
    const errors = await openSession(page, "sh-coordinator-sent-overdue", theme);
    await expect(page.getByTestId("sh-way-out-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("sh-way-out-text")).toContainText("first abandon the registration transaction in Activity");
    await page.getByTestId("sh-way-out-toggle").click();
    await expect(page.getByTestId("sh-way-out-text")).toHaveCount(0);
    await expect(page.getByTestId("sh-way-out-toggle")).toHaveAttribute("aria-expanded", "false");
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: after Send, the way out abandons the registration in Activity, then Abandon here spends the coins back`, async ({ page }, info) => {
    const errors = await openSession(page, "sh-coordinator-send", theme);
    await page.getByTestId("sh-send").click();
    await confirmWith(page, "send");
    await expect(page.getByTestId("sh-row-sent")).toBeVisible();
    await expect(page.getByTestId("sh-way-out-text")).toHaveCount(0);
    await page.getByTestId("sh-way-out-toggle").click();
    await expect(page.getByTestId("sh-way-out-text")).toContainText("first abandon the registration transaction in Activity");
    await expect(page.getByTestId("sh-abandon-button"), "the registration still spends the coins").toHaveCount(0);
    await expect(page.getByTestId("sh-way-out-abandon")).toHaveCount(0);
    if (info.project.name === "chromium") await shot(page, "sh-after-send-way-out", theme);
    await page.getByTestId("sh-way-out-activity").click();
    await expect(page.getByTestId("tx-route-filter")).toHaveAttribute("data-txids", "7e".repeat(32));
    await page.locator(`.tx-row[data-txid="${"7e".repeat(32)}"]`).first().click();
    await page.getByTestId("tx-abandon").click();
    await page.getByTestId("tx-abandon-confirm").click();
    await expect(page.getByTestId("tx-action-note")).toHaveText("The transaction was abandoned.");
    await page.getByTestId("tx-detail").locator(".sheet-close").click();
    await expect(page.getByTestId("tx-detail")).toBeHidden();
    await page.getByTestId("sidebar-masternodes").click();
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-in-progress").getByRole("button").first().click();
    await page.getByTestId("sh-way-out-toggle").click();
    await expect(page.getByTestId("sh-way-out-abandon")).toBeVisible();
    await expect(page.getByTestId("sh-abandon-button")).toBeVisible();
    await page.getByTestId("sh-way-out-abandon").click();
    await expect(page.getByTestId("sh-abandon")).toBeVisible();
    await page.getByTestId("sh-abandon-passphrase").fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("sh-abandon-understand").check();
    await page.getByTestId("sh-abandon-abandon").click();
    await expect(page.getByTestId("sh-abandon")).toBeHidden();
    await expect(page.getByTestId("sh-abandon-button")).toHaveCount(0);
    await page.getByTestId("sh-resend").click();
    await confirmWith(page, "resend");
    await expect(page.getByTestId("sh-resend-error")).toContainText("abandoned, so it can't be sent again");
    expect(errors).toEqual([]);
  });
}

test("a scrim press soon after a sheet opens keeps it; one later than 600 ms closes it", async ({ page }) => {
  const errors = await open(page, "sh-capability-active", "light");
  await page.getByTestId("mn-shared").click();
  const sheet = page.getByTestId("sh-start");
  // At once, on the scrim's top-left corner, outside the sheet: taken as the opening click's tail.
  await page.mouse.click(5, 5);
  await expect(sheet).toBeVisible();
  await page.waitForTimeout(800);
  await page.mouse.click(5, 5);
  await expect(sheet).toBeHidden();
  expect(errors).toEqual([]);
});
