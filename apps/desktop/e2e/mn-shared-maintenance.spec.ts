import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// A shared masternode's maintenance (MG-10 §2.5–§2.8, S6) on the fixture: Dissolve alone from its
// preview to the send, a standby created and saved behind MG's alert, a dissolution together approved
// and sent, a key change that never offers Send, and mainnet, where every send and the key change are absent and MG's line
// takes its place. DW_SHOTS=<dir> also writes each state's PNG.

const SHOTS = process.env.DW_SHOTS;
const SH_ROW = `mn-row-${"ec".repeat(32)}`;
const MN_NAME = "Masternode 4";
const BOB_SHARE = "Bob";
const EXPORT_ALERT =
  "Anyone who has this file can dissolve this masternode at any time. Every owner's share still goes back to its refund address, but the masternode stops earning. Keep the file private, and give it only to co-owners you trust.";
const PENALTY_ALERT = (who: string, penalty: string) =>
  `Anyone who has this file can dissolve this masternode at any time, and the masternode stops earning. The file also holds a line that pays the early-exit penalty: anyone who has it can send that line, even after the early period ends, and make ${who} pay ${penalty} to the other owners. Keep the file private, and give it only to co-owners you trust.`;
const REFUSED = "DashPay Desktop isn't connected to the Dash network, so this wasn't sent. Send it again from In progress.";
const DISSOLVE_LINE =
  "This version can't send shared-masternode transactions on mainnet yet. To leave now, create a standby dissolution and save it with Save a copy… Then run sendrawtransaction in Dash Core's console with the hex line under FULL PRINCIPAL. Before the early period ends, use the line under IMMEDIATE instead, which pays the early-exit penalty. You can still follow this masternode and approve steps here.";
const KEY_APPROVE_OFF = "This version can't change a shared masternode's keys on mainnet yet, so you can't approve this change here.";
const CLOSE_NOTHING =
  "None of this wallet's coins are reserved for this session. Keeping it lets you carry on where you left off. If you discard it while the others are still waiting for you, they have to start over.";
const SEND_NOTE = "This version can't send shared-masternode transactions on mainnet yet. You can still follow this masternode, approve steps and save a standby dissolution here.";

const go = async (page: Page, scenario: string, theme: Theme) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
};

/** The shared masternode's pane. */
const openPane = async (page: Page, scenario: string, theme: Theme) => {
  await go(page, scenario, theme);
  await page.getByTestId(SH_ROW).click();
  await expect(page.getByTestId("sh-detail")).toBeVisible();
};

/** The one session in progress, from Shared masternode ▸ Sessions in progress. */
const openSession = async (page: Page, scenario: string, theme: Theme) => {
  await go(page, scenario, theme);
  await page.getByTestId("mn-shared").click();
  await page.getByTestId("sh-in-progress").getByRole("button").first().click();
  await expect(page.getByTestId("sh-session")).toBeVisible();
};

const passphrase = (page: Page, field: string) => page.getByTestId(`${field}-passphrase`).fill(FIXTURE_PASSPHRASE);

const shot = async (page: Page, name: string, theme: Theme, chromium: boolean) => {
  if (SHOTS && chromium) await page.screenshot({ path: `${SHOTS}/${name}-${theme}.png` });
};

for (const theme of THEMES) {
  test(`shared maintenance ${theme}: Dissolve alone, preview to sent`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-detail", theme);
    await expect(page.getByTestId("sh-mainnet-dissolve-note")).toHaveCount(0);
    await page.getByTestId("sh-detail-actions").scrollIntoViewIfNeeded();
    await shot(page, "sh-maint-pane", theme, chromium);
    await page.getByTestId("sh-dissolve-open").click();
    await expect(page.getByTestId("sh-dissolve-figures")).toBeVisible();
    await expect(page.getByTestId("sh-dissolve-early")).toBeVisible();
    const leaves = page.getByTestId("sh-dissolve-leaves");
    await expect(leaves).toContainText("20.00001");
    // What leaves the wallet shows before the passphrase.
    const before = await leaves.boundingBox();
    const field = await page.getByTestId("sh-dissolve-credential-passphrase").boundingBox();
    expect(before!.y).toBeLessThan(field!.y);
    await passphrase(page, "sh-dissolve-credential");
    await expect(page.getByTestId("sh-dissolve-now")).toBeDisabled();
    await page.getByTestId("sh-dissolve-accept").locator("input").check();
    await expect(page.getByTestId("sh-dissolve-now")).toBeEnabled();
    await shot(page, "sh-dissolve-preview", theme, chromium);
    await page.getByTestId("sh-dissolve-now").click();
    await expect(page.getByTestId("sh-dissolve-sent")).toBeVisible();
    await shot(page, "sh-dissolve-sent", theme, chromium);
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: Together accepts an edited fee below its suggestion`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-detail", theme);
    await page.getByTestId("sh-dissolve-open").click();
    await page.getByTestId("sh-dissolve-together").click();
    const fee = page.getByTestId("sh-together-fee");
    await expect(fee).toHaveValue("0.00001000");
    await expect(page.getByTestId("sh-together-fee-help")).toHaveText("Suggested fee for 3 shares: 0.00001000 tDASH");
    await expect(page.getByTestId("sh-dissolve-together-form")).not.toContainText("duffs");
    await passphrase(page, "sh-together-credential");
    await expect(page.getByTestId("sh-together-start")).toBeEnabled();
    await shot(page, "sh-together-fee", theme, chromium);
    await fee.fill("0.000009");
    await expect(page.getByTestId("sh-together-start")).toBeEnabled();
    await shot(page, "sh-together-fee-low", theme, chromium);
    await page.getByTestId("sh-together-start").click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-row-togetherSendApproval")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: a standby created, then saved behind MG's alert`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-standby-none", theme);
    await page.getByTestId("sh-standby-prompt").scrollIntoViewIfNeeded();
    await expect(page.getByTestId("sh-standby-prompt")).toBeVisible();
    await shot(page, "sh-standby-prompt", theme, chromium);
    await page.getByTestId("sh-standby-create").click();
    await expect(page.getByTestId("sh-standby-create-sheet")).toBeVisible();
    await passphrase(page, "sh-standby-create-credential");
    await shot(page, "sh-standby-create", theme, chromium);
    await page.getByTestId("sh-standby-create-confirm").click();
    await expect(page.getByTestId("sh-standby-create-sheet")).toBeHidden();
    await expect(page.getByTestId("sh-standby-prompt")).toHaveCount(0);
    await page.getByTestId("sh-standby-save-0-1000").click();
    await expect(page.getByTestId("sh-standby-export")).toBeVisible();
    await expect(page.getByTestId("sh-export-alert")).toHaveText(PENALTY_ALERT("your share", "20.00000000 tDASH"));
    await expect(page.getByTestId("sh-export-save")).toHaveText("Save a copy…");
    await shot(page, "sh-standby-export", theme, chromium);
    await passphrase(page, "sh-export-credential");
    await page.getByTestId("sh-export-save").click();
    await expect(page.getByTestId("sh-export-saved")).toContainText(`standby-dissolution-${"ec".repeat(4)}-share1.txt`);
    await shot(page, "sh-standby-saved", theme, chromium);
    expect(errors).toEqual([]);
  });

  for (const handoff of ["refused", "unknown"] as const) test(`shared maintenance ${theme}: ${handoff} hand-off never reads as sent`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    const setHandoff = () => page.evaluate((kind) => {
      if (kind === "unknown") window.__dwFixture!.world.shared.handoff = "send.broadcast_unknown";
    }, handoff);
    const warning = handoff === "refused" ? REFUSED : "DashPay Desktop couldn't confirm that the dissolution reached the network. It may still confirm. You can send it again from In progress.";
    await openPane(page, "sh-handoff-refused", theme);
    await setHandoff();
    await page.getByTestId("sh-dissolve-open").click();
    await passphrase(page, "sh-dissolve-credential");
    await page.getByTestId("sh-dissolve-accept").locator("input").check();
    await page.getByTestId("sh-dissolve-now").click();
    await expect(page.getByTestId("sh-dissolve-refused")).toHaveText(warning);
    await expect(page.getByTestId("sh-dissolve-sent")).toHaveCount(0);
    await shot(page, `sh-dissolve-${handoff}`, theme, chromium);

    await openPane(page, "sh-handoff-refused", theme);
    await setHandoff();
    await page.getByTestId("sh-standby-send-0-1000").click();
    await passphrase(page, "sh-standby-send-credential");
    await page.getByTestId("sh-standby-send-confirm").click();
    await expect(page.getByTestId("sh-standby-refused")).toHaveText(warning);
    await expect(page.getByTestId("sh-standby-sent")).toHaveCount(0);
    await shot(page, `sh-standby-${handoff}`, theme, chromium);

    await openSession(page, "sh-handoff-refused", theme);
    await setHandoff();
    await passphrase(page, "sh-credential");
    await page.getByTestId("sh-send-maintenance").click();
    await expect(page.getByTestId("sh-together-refused")).toHaveText(warning);
    await expect(page.getByTestId("sh-row-sent")).toHaveCount(0);
    await expect(page.getByTestId("sh-send-maintenance")).toHaveCount(0);
    await shot(page, `sh-together-${handoff}`, theme, chromium);
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: only an early full refund: Create offered; a co-owner's file names their penalty`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-standby-full-early", theme);
    await page.getByTestId("sh-standby-prompt").scrollIntoViewIfNeeded();
    await expect(page.getByTestId("sh-standby-prompt")).toBeVisible();
    await shot(page, "sh-standby-full-early", theme, chromium);
    await page.getByTestId("sh-standby-save-0-1000").click();
    await expect(page.getByTestId("sh-export-alert")).toHaveText(EXPORT_ALERT);
    await page.getByTestId("sh-standby-export").getByRole("button", { name: /close/i }).first().click();
    await page.getByTestId("sh-standby-save-1-1000").click();
    await expect(page.getByTestId("sh-export-alert")).toHaveText(PENALTY_ALERT(BOB_SHARE, "20.00000000 tDASH"));
    await shot(page, "sh-standby-export-other", theme, chromium);
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: a dissolution together approved, then sent`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openSession(page, "sh-together-approve", theme);
    await expect(page.getByTestId("sh-row-togetherApprove")).toBeVisible();
    await expect(page.getByTestId("sh-stages")).toHaveCount(0);
    await expect(page.getByTestId("sh-masternode-name")).toHaveText(MN_NAME);
    await expect(page.getByTestId("sh-masternode-hash")).toHaveText("ec".repeat(32));
    await passphrase(page, "sh-credential");
    await shot(page, "sh-together-approve", theme, chromium);
    await page.getByTestId("sh-approve-maintenance").click();
    await expect(page.getByTestId("sh-row-togetherSend")).toBeVisible();
    await shot(page, "sh-together-send", theme, chromium);
    const field = page.getByTestId("sh-credential-passphrase");
    if (await field.isVisible()) await field.fill(FIXTURE_PASSPHRASE);
    await page.getByTestId("sh-send-maintenance").click();
    await expect(page.getByTestId("sh-row-sent")).toBeVisible();
    await shot(page, "sh-together-sent", theme, chromium);
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: an actor fee is approved only once accepted`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await openSession(page, "sh-together-actor-fee", theme);
    await passphrase(page, "sh-credential");
    await expect(page.getByTestId("sh-actor-fee")).toBeVisible();
    await expect(page.getByTestId("sh-approve-maintenance")).toBeDisabled();
    await page.getByTestId("sh-actor-fee-accept").locator("input").check();
    await expect(page.getByTestId("sh-approve-maintenance")).toBeEnabled();
    await shot(page, "sh-together-actor-fee", theme, info.project.name === "chromium");
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: a key change started, approved, and never sent`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-detail", theme);
    await page.getByTestId("sh-keychange-open").click();
    await expect(page.getByTestId("sh-keychange-start")).toBeDisabled();
    await page.getByTestId("sh-keychange-voting-keychain").locator("input").check();
    await passphrase(page, "sh-keychange-credential");
    await shot(page, "sh-keychange-start", theme, chromium);
    await page.getByTestId("sh-keychange-start").click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-row-keyWaiting")).toBeVisible();
    await expect(page.getByTestId("sh-key-voting")).toBeVisible();

    await openSession(page, "sh-keychange-approve", theme);
    await expect(page.getByTestId("sh-row-keyApprove")).toBeVisible();
    await expect(page.getByTestId("sh-masternode-hash")).toHaveText("ec".repeat(32));
    await expect(page.getByTestId("sh-key-operator")).not.toContainText("…");
    await shot(page, "sh-keychange-approve", theme, chromium);

    await openSession(page, "sh-keychange-combined", theme);
    await expect(page.getByTestId("sh-row-keySendLater")).toBeVisible();
    await expect(page.getByTestId("sh-send-maintenance")).toHaveCount(0);
    await expect(page.getByTestId("sh-session").getByRole("button", { name: /send/i })).toHaveCount(0);
    await shot(page, "sh-keychange-later", theme, chromium);
    await page.getByTestId("sh-close-button").click();
    await expect(page.getByTestId("sh-close")).toBeVisible();
    await shot(page, "sh-keychange-discard", theme, chromium);
    await page.getByTestId("sh-close-discard").click();
    await expect(page.getByTestId("sh-session")).toBeHidden();
    expect(errors).toEqual([]);
  });

  test(`shared maintenance ${theme}: on mainnet no send shows; MG's line and the note do`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    const chromium = info.project.name === "chromium";
    await openPane(page, "sh-detail-mainnet", theme);
    await expect(page.getByTestId("sh-mainnet-dissolve-note")).toHaveText(DISSOLVE_LINE);
    await expect(page.getByTestId("sh-detail").getByRole("button", { name: /send/i })).toHaveCount(0);
    await expect(page.getByTestId("sh-standby-save-0-1000")).toBeVisible();
    await expect(page.getByTestId("sh-dissolve-open")).toBeVisible();
    await expect(page.getByTestId("sh-keychange-open")).toHaveCount(0);
    await page.getByTestId("sh-mainnet-dissolve-note").scrollIntoViewIfNeeded();
    await shot(page, "sh-mainnet-pane", theme, chromium);
    await page.getByTestId("sh-dissolve-open").click();
    await expect(page.getByTestId("sh-dissolve").getByTestId("sh-mainnet-dissolve-note")).toHaveText(DISSOLVE_LINE);
    await expect(page.getByTestId("sh-dissolve-now")).toHaveCount(0);
    await page.getByTestId("sh-dissolve-standby").click();
    await expect(page.getByTestId("sh-dissolve").getByRole("button", { name: /send/i })).toHaveCount(0);
    await shot(page, "sh-mainnet-dissolve", theme, chromium);

    await openSession(page, "sh-together-combined-mainnet", theme);
    await expect(page.getByTestId("sh-row-togetherSendOff")).toBeVisible();
    await expect(page.getByTestId("sh-mainnet-note")).toHaveText(SEND_NOTE);
    await expect(page.getByTestId("sh-send-maintenance")).toHaveCount(0);
    await expect(page.getByTestId("sh-session").getByRole("button", { name: /send/i })).toHaveCount(0);
    await expect(page.getByTestId("sh-copy")).toHaveText("Copy signatures");
    await expect(page.getByTestId("sh-save")).toBeVisible();
    await shot(page, "sh-mainnet-session", theme, chromium);
    await page.getByTestId("sh-close-button").click();
    await expect(page.getByTestId("sh-close")).toContainText(CLOSE_NOTHING);
    await expect(page.getByTestId("sh-close-discard")).toHaveText("Discard");
    await shot(page, "sh-close-nothing", theme, chromium);

    await openSession(page, "sh-keychange-approve-mainnet", theme);
    await expect(page.getByTestId("sh-row-keyApproveOff")).toHaveText(KEY_APPROVE_OFF);
    await expect(page.getByTestId("sh-approve-maintenance")).toHaveCount(0);
    await expect(page.getByTestId("sh-credential-passphrase")).toHaveCount(0);
    await shot(page, "sh-mainnet-keychange", theme, chromium);
    expect(errors).toEqual([]);
  });
}
