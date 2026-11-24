import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// C-routes D-C3 (C3-8): the top-up sheet's "Pay with" on the fixture backend. Dash wallet is
// chosen every time; Platform pays from the wallet's Platform addresses with Max, its confirm
// copy and outcomes, a held top-up offering "Move to Dash wallet"; Shielded is listed and not
// offered yet. `DW_SHOTS_DIR` also writes each state's screenshot for the pull request.

// Each walk runs two or three axe scans, which take WebKit most of the default budget.
test.describe.configure({ timeout: 60_000 });

const sheet = (page: Page) => page.getByTestId("topup-body");
const shots = process.env.DW_SHOTS_DIR;

async function openTopUp(page: Page, theme: Theme, nextTopUp?: "held" | "pending") {
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
}
