import { expect, test, type Page } from "@playwright/test";
import { SH_TEXT } from "../src/fixture/shared";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// Shared masternodes (MG-10b S5) on the fixture: Start or join, the coordinator's setup to its
// invitation, the session sheet's stages, a pasted invitation joined, and Close offering Discard only
// while the engine reads the session as discardable. DW_SHOTS=<dir> also writes each state's PNG.

const SHOTS = process.env.DW_SHOTS;

const open = async (page: Page, scenario: string, theme: Theme) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
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

  test(`shared masternode ${theme}: a pasted invitation is joined; Close offers Discard`, async ({ page }, info) => {
    const errors = await open(page, "sh-capability-active", theme);
    await page.getByTestId("mn-shared").click();
    await page.getByTestId("sh-paste").fill(SH_TEXT.invitation);
    await page.getByTestId("sh-read").click();
    await expect(page.getByTestId("sh-session")).toBeVisible();
    await expect(page.getByTestId("sh-join-button")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-join", theme);
    await page.getByTestId("sh-join-button").click();
    await expect(page.getByTestId("sh-copy")).toBeVisible();
    await page.getByTestId("sh-close-button").click();
    await expect(page.getByTestId("sh-close")).toBeVisible();
    if (info.project.name === "chromium") await shot(page, "sh-close", theme);
    await page.getByTestId("sh-close-discard").click();
    await expect(page.getByTestId("sh-close")).toBeHidden();
    expect(errors).toEqual([]);
  });

  test(`shared masternode ${theme}: locked terms to approve, and a session that may be out has no Discard`, async ({ page }, info) => {
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
    await expect(page.getByTestId("sh-copy")).toHaveCount(0);
    await expect(page.getByTestId("sh-row-signedOut")).toContainText("can't be discarded");
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
}
