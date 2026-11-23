import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP6-02: withdraw credits from My Profile on the fixture backend, by parity ID IOS-070 (withdraw
// identity credits to Core): the sheet's form (Max or an amount, the destination), the quote
// (credits taken, the fee, the Dash expected), the grant prompt, the outcome, the refusals, and
// the recent withdrawals under the credits.

async function openProfile(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("page-profile")).toBeVisible();
  return errors;
}

const sheet = (page: Page) => page.getByTestId("withdraw-body");
const RECEIVE = /^y[1-9A-HJ-NP-Za-km-z]{33}$/;
const MAINNET = "XmNfXq2kDmrNBTiDTofohRemwGur1WmgTT";
/** The fixture's first withdrawal document (fixture/topup.ts `withdrawalId`). */
const DOCUMENT = "Wd0001".padEnd(44, "x");
const PLATFORM = "tdash1kzc0lmmvxmfkd8ztchz4m6j8lgdjn4pxc0quhs7";

/** Opens the sheet and goes to the quote: Max, or `amount` in tDASH. */
async function toQuote(page: Page, amount: string | null = null) {
  await page.getByTestId("profile-withdraw").click();
  await expect(sheet(page)).toHaveAttribute("data-step", "form");
  await expect(page.getByTestId("withdraw-to")).toHaveValue(RECEIVE);
  if (amount !== null) {
    await page.getByTestId("withdraw-other").click();
    await page.getByTestId("withdraw-amount").fill(amount);
  }
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-quote")).toBeVisible();
}

for (const theme of THEMES) {
  test(`IOS-070 withdraw sheet ${theme}: form, quote, grant, outcome; 0 serious axe issues`, async ({ page }) => {
    const errors = await openProfile(page, "dashpay-registered", theme);
    await page.getByTestId("profile-withdraw").click();
    await expect(page.getByTestId("withdraw-next")).toBeEnabled();
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("withdraw-next").click();
    await expect(page.getByTestId("withdraw-quote")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("withdraw-go").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-step", "done");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test(`IOS-070 withdraw refusals and recent withdrawals ${theme}: 0 serious axe issues`, async ({ page }) => {
    const errors = await openProfile(page, "dashpay-withdrawals", theme, "&fail=credits_withdraw:keysMissing");
    await expect(page.getByTestId("profile-withdrawal")).toHaveCount(4);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("profile-withdraw").click();
    await page.getByTestId("withdraw-to").fill(PLATFORM);
    await expect(page.getByTestId("withdraw-to-problem")).not.toBeEmpty();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("withdraw-cancel").click();
    await toQuote(page);
    await page.getByTestId("withdraw-go").click();
    await authorize(page);
    await expect(sheet(page)).toHaveAttribute("data-step", "refused");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("IOS-070 D1: Max withdraws everything but the fee to the receive address; the balance and the list follow", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  const before = await page.getByTestId("profile-credits-figure").textContent();
  await expect(page.getByTestId("profile-withdrawal")).toHaveCount(0);
  await toQuote(page);
  // 1.845 tDASH of credits: 1.841 out, 0.004 kept for the fee, 1.8409981 paid after Core's fee.
  await expect(page.getByTestId("withdraw-quote-credits")).toHaveText("1.841 tDASH");
  await expect(page.getByTestId("withdraw-quote-fee")).toHaveText("0.004 tDASH");
  await expect(page.getByTestId("withdraw-quote-duffs")).toHaveText("1.8409981 tDASH");
  await expect(page.getByTestId("withdraw-quote-to")).toHaveText(RECEIVE);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-outcome-title")).toHaveText("Withdrawal queued");
  await expect(page.getByTestId("withdraw-outcome-text")).toContainText("1.8409981 tDASH is on its way to y");
  await expect(page.getByTestId("withdraw-outcome-text")).toContainText("Your credit balance is 0.00 tDASH.");
  await page.getByTestId("withdraw-done").click();
  await expect(page.getByTestId("withdraw-sheet")).toHaveCount(0);
  await expect(page.getByTestId("profile-credits-figure")).not.toHaveText(before!);
  await expect(page.getByTestId("profile-withdrawal")).toHaveCount(1);
  await expect(page.getByTestId("profile-withdrawal")).toHaveAttribute("data-pending", "true");
  await expect(page.getByTestId("profile-withdrawal")).toContainText("Withdrawal of 1.8409981 tDASH");
  await expect(page.getByTestId("profile-withdrawal")).toContainText("Pending");
});

test("IOS-070: a partial withdrawal to a pasted address quotes the amount as entered", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-withdraw").click();
  await page.getByTestId("withdraw-other").click();
  await page.getByTestId("withdraw-amount").fill("0.5");
  await page.getByTestId("withdraw-to").fill("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-quote-credits")).toHaveText("0.50 tDASH");
  await expect(page.getByTestId("withdraw-quote-duffs")).toHaveText("0.4999981 tDASH");
  await expect(page.getByTestId("withdraw-quote-to")).toHaveText("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-outcome-text")).toContainText("0.4999981 tDASH is on its way to yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
  // The balance less the amount and the fee.
  await expect(page.getByTestId("withdraw-outcome-text")).toContainText("Your credit balance is 1.341 tDASH.");
});

test("IOS-070: the form checks the amount and the address before Continue", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-withdraw").click();
  await page.getByTestId("withdraw-other").click();
  await page.getByTestId("withdraw-amount").fill("abc");
  await expect(page.getByTestId("withdraw-amount-problem")).toHaveText("Enter an amount.");
  await expect(page.getByTestId("withdraw-next")).toBeDisabled();
  await page.getByTestId("withdraw-amount").fill("0.1");
  await expect(page.getByTestId("withdraw-amount-problem")).toBeEmpty();
  await page.getByTestId("withdraw-to").fill(MAINNET);
  await expect(page.getByTestId("withdraw-to-problem")).toHaveText("Enter a Dash address.");
  await expect(page.getByTestId("withdraw-next")).toBeDisabled();
  await page.getByTestId("withdraw-to").fill(PLATFORM);
  await expect(page.getByTestId("withdraw-to-problem")).toHaveText("This is a Dash Platform address. Withdraw to a Dash address.");
  await page.getByTestId("withdraw-to").fill("yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
  await expect(page.getByTestId("withdraw-to-problem")).toBeEmpty();
  await expect(page.getByTestId("withdraw-next")).toBeEnabled();
});

test("IOS-070: the quote refuses an amount outside Platform's range or past the balance; Back returns to the form", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  await page.getByTestId("profile-withdraw").click();
  await page.getByTestId("withdraw-other").click();
  await page.getByTestId("withdraw-amount").fill("0.000001");
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-quote-error")).toHaveText("Withdraw between 0.0000119 tDASH and 500.00 tDASH.");
  await expect(page.getByTestId("withdraw-go")).toBeDisabled();
  await page.getByTestId("withdraw-back").click();
  await page.getByTestId("withdraw-amount").fill("1.845");
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-quote-error")).toContainText("Not enough credits: 1.849 tDASH needed with the fee");
});

test("IOS-070 D7, D23: an unanswered withdrawal is not retried from the sheet and stays unconfirmed on My Profile", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-withdraw-lost");
  await toQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-refusal")).toHaveText("It may have been sent. Check your withdrawals in My Profile before trying again.");
  await expect(page.getByTestId("withdraw-document")).toHaveText(`Withdrawal document: ${DOCUMENT}`);
  await expect(page.getByTestId("withdraw-refused-back")).toHaveCount(0);
  await page.getByTestId("withdraw-close").click();
  await expect(page.getByTestId("withdraw-sheet")).toHaveCount(0);
  // Platform does not list it: My Profile keeps it, and a second withdrawal is warned.
  await expect(page.getByTestId("profile-withdraw-unconfirmed")).toContainText("Withdrawal not on Dash Platform yet");
  await expect(page.getByTestId("profile-withdraw-unconfirmed")).toContainText(DOCUMENT);
  await expect(page.getByTestId("profile-withdrawal")).toHaveCount(0);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("profile-withdraw").click();
  await expect(page.getByTestId("withdraw-unconfirmed")).toHaveText(
    "Your last withdrawal is not on Dash Platform yet and may still go through. If you withdraw again, at most one of the two goes through.",
  );
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
});

test("IOS-070 D24: a withdrawal after an unconfirmed one that went through is refused, naming it", async ({ page }) => {
  const errors = await openProfile(page, "dashpay-withdraw-lost-landed");
  await toQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-document")).toHaveText(`Withdrawal document: ${DOCUMENT}`);
  await page.getByTestId("withdraw-close").click();
  await expect(page.getByTestId("profile-withdraw-unconfirmed")).toContainText(DOCUMENT);
  // The next one reads it back first: it went through, so nothing is sent.
  await page.getByTestId("profile-withdraw").click();
  await expect(page.getByTestId("withdraw-unconfirmed")).toBeVisible();
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-go")).toBeEnabled();
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-refusal")).toHaveText(
    "Your last withdrawal went through after all, so this one does not. Withdraw again only if you want a second one.",
  );
  await expect(page.getByTestId("withdraw-document")).toHaveText(`Withdrawal document: ${DOCUMENT}`);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("withdraw-close").click();
  await expect(page.getByTestId("profile-withdraw-unconfirmed")).toHaveCount(0);
  await expect(page.getByTestId("profile-withdrawal")).toHaveCount(1);
  expect(errors).toEqual([]);
});

test("IOS-070: a withdrawal read back after an unanswered broadcast says the balance updates", async ({ page }) => {
  await openProfile(page, "dashpay-withdraw-landed");
  await toQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-outcome-text")).toContainText("Your credit balance updates shortly.");
});

test("IOS-070: no transfer key refuses with its reason", async ({ page }) => {
  await openProfile(page, "dashpay-registered", "light", "&fail=credits_withdraw:keysMissing");
  await toQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(page.getByTestId("withdraw-refusal")).toHaveText("This identity has no transfer key in this wallet, so it cannot withdraw.");
});

test("IOS-070 D11: recent withdrawals show their state; Withdraw needs a known balance", async ({ page }) => {
  await openProfile(page, "dashpay-withdrawals");
  const rows = page.getByTestId("profile-withdrawal");
  await expect(rows).toHaveCount(4);
  await expect(rows.nth(0)).toContainText("Withdrawal of 0.4999981 tDASH");
  await expect(rows.nth(0)).toContainText("Pending");
  await expect(rows.nth(1)).toContainText("Pending");
  await expect(rows.nth(2)).toContainText("Sent");
  await expect(rows.nth(3)).toContainText("Failed: the credits were not returned");
  await openProfile(page, "dashpay-credits-unknown");
  await expect(page.getByTestId("profile-withdraw")).toBeDisabled();
});

test("IOS-070: a withdrawal that ends while the sheet is closed is offered on the page and shown once", async ({ page }) => {
  await openProfile(page, "dashpay-registered");
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("credits_withdraw"));
  await toQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
  await expect(sheet(page)).toHaveAttribute("data-step", "running");
  await page.getByTestId("withdraw-sheet").getByRole("button", { name: "Close" }).click();
  await expect(page.getByTestId("profile-withdraw")).toHaveText("Withdrawing…");
  await expect(page.getByTestId("profile-withdraw")).toBeDisabled();
  await release.evaluate((r) => r());
  await expect(page.getByTestId("profile-withdraw-unseen")).toContainText("Your withdrawal has an update.");
  await page.getByTestId("profile-withdraw-show").click();
  await expect(page.getByTestId("withdraw-outcome-title")).toHaveText("Withdrawal queued");
  await page.getByTestId("withdraw-done").click();
  await expect(page.getByTestId("profile-withdraw-unseen")).toHaveCount(0);
});
