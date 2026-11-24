import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, openTransfer, THEMES, trackPageErrors, type Theme } from "./helpers";

// C-routes D-C3 (C3-8): the top-up sheet's "Pay with" on the fixture backend. Dash wallet is
// chosen every time; Platform pays from the wallet's Platform addresses with Max, its confirm
// copy and outcomes, a held top-up offering "Move to Dash wallet"; Shielded is listed and not
// offered yet. `DW_SHOTS_DIR` also writes each state's screenshot for the pull request.

// Each walk runs two or three axe scans, which take WebKit most of the default budget.
test.describe.configure({ timeout: 60_000 });

const sheet = (page: Page) => page.getByTestId("topup-body");
const shots = process.env.DW_SHOTS_DIR;

async function openTopUp(page: Page, theme: Theme, nextTopUp?: "held" | "pending" | "failed") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", theme));
  await page.getByTestId("toolbar").waitFor();
  if (nextTopUp) await page.evaluate((n) => void (window.__dwFixture!.world.pending.platform!.nextTopUp = n), nextTopUp);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("page-profile")).toBeVisible();
  await page.getByTestId("profile-top-up").click();
  await expect(sheet(page)).toHaveAttribute("data-step", "amount");
  return errors;
}

async function shot(page: Page, theme: Theme, state: string) {
  if (!shots) return;
  await fontsReady(page);
  await page.getByTestId("topup-sheet").screenshot({ path: `${shots}/${process.env.DW_SHOTS_TAG ?? "after"}-${theme}-${state}.png` });
}

async function moveShot(page: Page, theme: Theme, state: string) {
  if (!shots) return;
  await fontsReady(page);
  await page.getByTestId("transfer-sheet").screenshot({ path: `${shots}/${process.env.DW_SHOTS_TAG ?? "after"}-${theme}-${state}.png` });
}

/** A top-up from Platform held, its outcome open with Move to Dash wallet. */
async function heldTopUp(page: Page, theme: Theme) {
  const errors = await openTopUp(page, theme, "held");
  await page.getByTestId("topup-source-platform").click();
  await page.getByTestId("topup-next").click();
  await page.getByTestId("topup-pay").click();
  await authorize(page);
  await expect(page.getByTestId("topup-move-to-core")).toBeVisible();
  return errors;
}

/** Home's transfer at Platform → Dash wallet's confirmation, its quote read. */
async function moveConfirm(page: Page) {
  await expect(page.getByTestId("transfer-sheet")).toBeVisible();
  await page.getByTestId("transfer-continue").click();
  await expect(page.getByTestId("platform-move-quote")).toBeVisible();
}

/** The held top-up lands: it is done and has taken its inputs. */
const topUpLands = (page: Page) =>
  page.evaluate(() => {
    const p = window.__dwFixture!.world.pending.platform!;
    const held = p.activity.find((a) => a.kind === "toIdentity" && a.mayLand)!;
    const inputs = p.heldInputs?.[held.id] ?? [];
    p.activity = p.activity.map((a) => (a.id === held.id ? { ...a, state: "done" as const, mayLand: false } : a));
    p.addresses = p.addresses.filter((a) => !inputs.includes(a.address));
  });

for (const theme of THEMES) {
  test(`Pay with: Dash wallet by default, Platform with Max, Shielded not offered (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme);
    await expect(page.getByTestId("topup-source-core").locator("input")).toBeChecked();
    await expect(page.getByTestId("topup-source-shielded")).toContainText("Not available on this network yet.");
    await expect(page.getByTestId("topup-source-shielded").locator("input")).toBeDisabled();
    await expect(page.getByTestId("topup-max")).toHaveCount(0);
    await shot(page, theme, "pay-with-dash-wallet");
    expect(await seriousAxeViolations(page)).toEqual([]);

    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-max").click();
    await shot(page, theme, "pay-with-platform-max");
    await page.getByTestId("topup-preset-5000000").click();
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toContainText("Estimated Platform fee");
    await expect(page.getByTestId("topup-platform-message")).toContainText("Top up @alice with 0.05");
    await shot(page, theme, "platform-confirm");
    expect(await seriousAxeViolations(page)).toEqual([]);

    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-outcome", "done");
    await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
    await shot(page, theme, "platform-done");
    await page.getByTestId("topup-done").click();

    // Reopened: Dash wallet again.
    await page.getByTestId("profile-top-up").click();
    await expect(page.getByTestId("topup-source-core").locator("input")).toBeChecked();
    expect(errors).toEqual([]);
  });

  test(`a held top-up from Platform offers Move to Dash wallet and holds the source (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme, "held");
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toBeVisible();
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(page.getByTestId("topup-outcome-title")).toHaveText("The credits were not added yet");
    await expect(page.getByTestId("topup-outcome-text")).toContainText("move these credits to your Dash wallet");
    await expect(page.getByTestId("topup-move-to-core")).toBeVisible();
    await shot(page, theme, "platform-held");
    expect(await seriousAxeViolations(page)).toEqual([]);

    // Move to Dash wallet: Home's transfer, at the move to the Dash wallet.
    await page.getByTestId("topup-move-to-core").click();
    await expect(page.getByTestId("page-home")).toBeVisible();
    await expect(page.getByTestId("transfer-sheet")).toBeVisible();
    await page.keyboard.press("Escape");

    // Until it settles the Platform source says why it can't pay.
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
    await page.getByTestId("profile-top-up").click();
    await expect(page.getByTestId("topup-source-platform")).toContainText("This top-up hasn't gone through yet");
    await expect(page.getByTestId("topup-source-platform").locator("input")).toBeDisabled();
    await expect(page.getByTestId("topup-source-move-to-core")).toBeVisible();
    await shot(page, theme, "pay-with-platform-held");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("topup-source-move-to-core").click();
    await expect(page.getByTestId("transfer-sheet")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`Max from Platform pays the amount Confirm showed (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme);
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-max").click();
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toBeVisible();
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-outcome", "done");
    await expect(page.getByTestId("topup-outcome-title")).toHaveText("Credits added");
    expect(errors).toEqual([]);
  });

  test(`in discreet mode Max from Platform shows no amount on Confirm (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "toggleDiscreet" }));
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-max").click();
    await page.getByTestId("topup-next").click();
    await expect(page.getByTestId("topup-quote")).toBeVisible();
    await shot(page, theme, "platform-confirm-max-discreet");
    const amountRow = page.getByTestId("topup-quote").locator(".detail-row").first().locator(".detail-value");
    expect((await amountRow.textContent())!.replace(/\D/g, "")).toBe("");
    // Masked amounts; only the fee address keeps its digits.
    await expect(page.getByTestId("topup-platform-message")).toContainText("with #.## from your Platform addresses? About #.## leaves them");
    expect(errors).toEqual([]);
  });

  test(`a failed top-up from Platform says nothing left, and History shows the top-up row (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme, "failed");
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-next").click();
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(page.getByTestId("topup-outcome-text")).toHaveText("Nothing left your Platform addresses.");
    await expect(page.getByTestId("topup-move-to-core")).toHaveCount(0);
    await shot(page, theme, "platform-failed");
    await page.getByTestId("topup-done").click();
    await page.getByTestId("profile-top-up").click();
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-next").click();
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-outcome", "done");
    await page.getByTestId("topup-done").click();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "activity" }));
    const row = page.locator('[data-txid^="platform:toIdentity:"]', { hasText: "Credit top-up from Platform" }).first();
    await expect(row).toBeVisible();
    if (shots) {
      await fontsReady(page);
      await row.screenshot({ path: `${shots}/${process.env.DW_SHOTS_TAG ?? "after"}-${theme}-history-top-up-row.png` });
    }
    expect(errors).toEqual([]);
  });

  test(`a pending top-up from Platform says it may still go through (${theme})`, async ({ page }) => {
    const errors = await openTopUp(page, theme, "pending");
    await page.getByTestId("topup-source-platform").click();
    await page.getByTestId("topup-next").click();
    await page.getByTestId("topup-pay").click();
    await authorize(page);
    await expect(page.getByTestId("topup-outcome-title")).toHaveText("Top-up sent");
    await expect(page.getByTestId("topup-move-to-core")).toHaveCount(0);
    await shot(page, theme, "platform-pending");
    expect(errors).toEqual([]);
  });

  test(`the exit of a held top-up that landed since its quote is refused, from the top-up sheet (${theme})`, async ({ page }) => {
    const errors = await heldTopUp(page, theme);
    await page.getByTestId("topup-move-to-core").click();
    await moveConfirm(page);
    await topUpLands(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toHaveText("The balance on your Platform addresses changed since you reviewed this move. Review it again.");
    await expect(page.getByTestId("platform-move-refused-back")).toBeVisible();
    expect(await page.evaluate(() => window.__dwFixture!.world.pending.platform!.activity.some((a) => a.kind === "moveToCore"))).toBe(false);
    await moveShot(page, theme, "exit-quote-changed");
    expect(await seriousAxeViolations(page)).toEqual([]);
    // Back: a new quote, no longer the exit of anything.
    await page.getByTestId("platform-move-refused-back").click();
    await expect(page.getByTestId("platform-move-quote")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`the exit of a held top-up that landed since its quote is refused, from Home (${theme})`, async ({ page }) => {
    const errors = await heldTopUp(page, theme);
    await page.getByTestId("topup-done").click();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
    await openTransfer(page, "platformToCore");
    await moveConfirm(page);
    await topUpLands(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toHaveText("The balance on your Platform addresses changed since you reviewed this move. Review it again.");
    expect(errors).toEqual([]);
  });

  test(`an exit's outcome shows the amount its Confirm showed, which it moved (${theme})`, async ({ page }) => {
    const errors = await heldTopUp(page, theme);
    await page.getByTestId("topup-move-to-core").click();
    await moveConfirm(page);
    const quoted = await page.getByTestId("platform-move-quote-duffs").innerText();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
    await expect(page.getByTestId("platform-move-outcome-text")).toContainText("is on its way to your Dash wallet");
    expect(await page.getByTestId("platform-move-outcome-text").innerText()).toContain(quoted);
    expect(errors).toEqual([]);
  });

  test(`an exit whose inputs no longer carry its amount moves nothing (${theme})`, async ({ page }) => {
    const errors = await heldTopUp(page, theme);
    await page.getByTestId("topup-move-to-core").click();
    await moveConfirm(page);
    // The held input holds less at submit than the quote read.
    await page.evaluate(() => {
      const p = window.__dwFixture!.world.pending.platform!;
      const held = p.activity.find((a) => a.kind === "toIdentity" && a.mayLand)!;
      const inputs = p.heldInputs?.[held.id] ?? [];
      p.addresses = p.addresses.map((a) => (inputs.includes(a.address) ? { ...a, credits: String(BigInt(a.credits) / 2n) } : a));
    });
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toHaveText("The balance on your Platform addresses changed since you reviewed this move. Review it again.");
    expect(errors).toEqual([]);
  });

  test(`the exit of a held top-up that executed in its place finds its credits used (${theme})`, async ({ page }) => {
    const errors = await heldTopUp(page, theme);
    await page.evaluate(() => void (window.__dwFixture!.world.pending.platform!.nextMove = "lost"));
    await page.getByTestId("topup-move-to-core").click();
    await moveConfirm(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("The move did not go through");
    await expect(page.getByTestId("platform-move-outcome-text")).toHaveText("These credits were already used: either the top-up went through, or an earlier move to your Dash wallet took them. Activity shows which.");
    await moveShot(page, theme, "exit-lost");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}
