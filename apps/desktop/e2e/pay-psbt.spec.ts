import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// P15 on the fixture backend: Pay ▸ To an address holds Advanced: unsigned transaction (PSBT),
// closed by default, which reviews the payment as entered as a PSBT only and opens it in Tools ▸
// PSBT to copy or save. A watch-only wallet starts with it open, and its Review makes the PSBT.

test.describe.configure({ timeout: 90_000 });

async function open(page: Page, scenario: string, theme: Theme) {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) },
    });
  });
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-pay").click();
  await fontsReady(page);
  return errors;
}

const copied = (page: Page) => page.evaluate(() => (window as unknown as { __copied?: string }).__copied);
const callCount = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.length);
const callsSince = (page: Page, from: number) => page.evaluate((n) => window.__dwFixture!.calls.slice(n), from);
const SIGNS_OR_SENDS = ["authorize", "draft_prepare", "draft_broadcast", "sign_psbt", "broadcast_psbt"];
/** A Mainnet address, which the Testnet fixture wallet cannot pay. */
const MAINNET_PAYEE = "XXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";

/** Holds the next call of `cmd` until the returned function is called (a late reply). */
async function hold(page: Page, cmd: "analyze_psbt") {
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), cmd);
  return () => release.evaluate((r) => r());
}

async function enter(page: Page) {
  await page.getByTestId("pay-address").fill(PAYEE);
  await page.getByTestId("pay-amount").fill("0.5");
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
}

/** Tools ▸ PSBT shows the payment's PSBT, and the unsigned action from `from` on made it without
 * signing or sending anything. */
async function landsInPsbt(page: Page, from: number) {
  await expect(page.getByTestId("tools-tabs-psbt")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("psbt-message")).toHaveText("The PSBT has been copied to the clipboard. You can also save it.");
  await expect(page.getByTestId("psbt-lines")).toContainText(`Sends 0.50000000 tDASH to ${PAYEE}`);
  await expect(page.getByTestId("psbt-copy")).toBeEnabled();
  await expect(page.getByTestId("psbt-save")).toBeEnabled();
  expect(await copied(page)).toMatch(/^[A-Za-z0-9+/]+=*$/);
  const calls = await callsSince(page, from);
  expect(calls).toEqual(expect.arrayContaining(["draft_create_unsigned", "analyze_psbt"]));
  expect(calls.filter((c) => SIGNS_OR_SENDS.includes(c))).toEqual([]);
  expect(await seriousAxeViolations(page)).toEqual([]);
}

for (const theme of THEMES) {
  test(`P15 ${theme}: Advanced's unsigned transaction reviews the payment as a PSBT and opens it to copy or save`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    const advanced = page.getByTestId("pay-advanced-psbt");
    await expect(advanced).not.toHaveAttribute("open");
    await expect(page.getByTestId("pay-continue")).toHaveText("Review");
    await advanced.locator("summary").click();
    await expect(advanced).toHaveAttribute("open", "");
    const create = page.getByTestId("pay-create-psbt");
    await expect(create).toBeDisabled();
    await expect(create).toHaveAttribute("title", "Enter an address and an amount first.");
    await expect(page.getByTestId("pay-psbt-note")).toHaveText(
      "Nothing is signed or sent. You can copy or save the PSBT and sign it in another wallet, for example an offline Dash Core wallet.",
    );
    await enter(page);
    await expect(create).toBeEnabled();
    expect(await seriousAxeViolations(page)).toEqual([]);

    await create.click();
    const sheet = page.getByTestId("pay-confirm");
    await expect(sheet.getByRole("heading", { name: "Unsigned transaction" })).toBeVisible();
    await expect(sheet).toContainText("Nothing is signed or sent: this creates an unsigned transaction (PSBT)");
    await expect(page.getByTestId("pay-confirm-send")).toHaveCount(0);
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
    await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.50000000");
    await expect(page.getByTestId("pay-confirm-unsigned")).toHaveClass(/dash-button-filledBlue/);
    expect(await seriousAxeViolations(page)).toEqual([]);

    const from = await callCount(page);
    await page.getByTestId("pay-confirm-unsigned").click();
    await landsInPsbt(page, from);
    expect(errors).toEqual([]);
  });

  test(`P15 ${theme}: a watch-only wallet starts on the unsigned path, and its Review makes the PSBT`, async ({ page }) => {
    const errors = await open(page, "watch-only", theme);
    const advanced = page.getByTestId("pay-advanced-psbt");
    await expect(advanced).toHaveAttribute("open", "");
    await expect(page.getByTestId("pay-psbt-note")).toContainText("This wallet is watch-only, so it cannot sign.");
    await expect(page.getByTestId("pay-create-psbt")).toHaveClass(/dash-button-filledBlue/);
    await expect(page.getByTestId("pay-continue")).toHaveText("Review PSBT");
    await enter(page);
    expect(await seriousAxeViolations(page)).toEqual([]);

    await page.getByTestId("pay-continue").click();
    const sheet = page.getByTestId("pay-confirm");
    await expect(sheet.getByRole("heading", { name: "Unsigned transaction" })).toBeVisible();
    await expect(page.getByTestId("pay-confirm-send")).toHaveCount(0);
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
    await page.getByTestId("pay-confirm-cancel").click();
    await expect(sheet).toHaveCount(0);

    await page.getByTestId("pay-create-psbt").click();
    const from = await callCount(page);
    await page.getByTestId("pay-confirm-unsigned").click();
    await landsInPsbt(page, from);
    expect(errors).toEqual([]);
  });
}

test("P15: after Advanced's unsigned review is cancelled, Review confirms a payment to send again", async ({ page }) => {
  await open(page, "funded", "light");
  await page.getByTestId("pay-advanced-psbt").locator("summary").click();
  await enter(page);
  await page.getByTestId("pay-create-psbt").click();
  await expect(page.getByTestId("pay-confirm-send")).toHaveCount(0);
  await page.getByTestId("pay-confirm-cancel").click();
  await page.getByTestId("pay-continue").click();
  const sheet = page.getByTestId("pay-confirm");
  await expect(sheet.getByRole("heading", { name: "Confirm payment" })).toBeVisible();
  await expect(page.getByTestId("pay-confirm-send")).toBeVisible();
  await expect(page.getByTestId("pay-confirm-unsigned")).toHaveClass(/dash-button-tintedBlue/);
});

test("P15: a PSBT that arrives after the user left Pay is neither copied nor opened in Tools", async ({ page }) => {
  await open(page, "funded", "light");
  await page.getByTestId("pay-advanced-psbt").locator("summary").click();
  await enter(page);
  await page.getByTestId("pay-create-psbt").click();
  const release = await hold(page, "analyze_psbt");
  await page.getByTestId("pay-confirm-unsigned").click();
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("analyze_psbt"))).toBe(true);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "activity" }));
  await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
  const leftAt = await page.evaluate(() => location.href);
  await release();
  await page.evaluate(() => new Promise((r) => setTimeout(() => requestAnimationFrame(() => r(null)), 300)));
  expect(await copied(page)).toBeUndefined();
  await expect(page.getByTestId("tools-tabs")).toHaveCount(0);
  expect(await page.evaluate(() => location.href)).toBe(leftAt);
});

test("P15: closing the unsigned review gives the focus back to Advanced, and Return does not start a payment", async ({ page }) => {
  await open(page, "funded", "light");
  await page.getByTestId("pay-advanced-psbt").locator("summary").click();
  await enter(page);
  await page.getByTestId("pay-create-psbt").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("pay-confirm")).toHaveCount(0);
  await expect
    .poll(() => page.evaluate(() => {
      const a = document.activeElement;
      return a?.getAttribute("data-testid") === "pay-create-psbt" || (a?.tagName === "SUMMARY" && !!a.closest('[data-testid="pay-advanced-psbt"]'));
    }))
    .toBe(true);
  await page.keyboard.press("Enter");
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await expect(page.getByTestId("pay-confirm-send")).toHaveCount(0);
  await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
});

for (const theme of THEMES) {
  test(`P15 ${theme}: with an address of another network, Create unsigned names the address's problem`, async ({ page }) => {
    await open(page, "funded", theme);
    await page.getByTestId("pay-advanced-psbt").locator("summary").click();
    await page.getByTestId("pay-address").fill(MAINNET_PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    const create = page.getByTestId("pay-create-psbt");
    await expect(create).toBeDisabled();
    await expect(create).toHaveAttribute("title", "This address is not valid on this network.");
    await expect(page.locator(".dash-button-why")).toHaveText("This address is not valid on this network.");
    expect(await seriousAxeViolations(page)).toEqual([]);
  });
}
