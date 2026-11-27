import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, THEMES, type Theme } from "./helpers";

const toggle = (page: Page) => page.getByRole("checkbox", { name: "Enable experimental gift cards" });

async function quote(page: Page, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme, "&network=mainnet"));
  await expect(page.getByTestId("sidebar-home")).toBeVisible();
  await expect(page.getByTestId("sidebar-explore")).toHaveCount(0);
  await page.getByTestId("sidebar-settings").click();
  await page.getByTestId("settings-subtract-fee").check();
  await toggle(page).check({ timeout: 3000 });
  await page.getByTestId("sidebar-explore").click();
  await page.getByLabel("First name").fill("Fixture");
  await page.getByLabel("Last name").fill("Buyer");
  await page.getByLabel("US state").selectOption("CA");
  await page.getByLabel("Email", { exact: true }).fill("fixture@example.invalid");
  await page.getByRole("button", { name: "Send email code" }).click();
  await page.getByLabel("Email code", { exact: true }).fill("123456");
  await page.getByRole("button", { name: "Verify email" }).click();
  await page.getByLabel("Search merchants").fill("Groceries");
  await expect(page.getByRole("button", { name: "Fixture Books" })).toHaveCount(0);
  await page.getByRole("button", { name: "Fixture Groceries" }).click();
  await page.getByLabel("Card value (USD)").selectOption("25.00");
  await page.getByRole("button", { name: "Get payment quote" }).click();
}

async function openConfirmation(page: Page) {
  await page.getByRole("button", { name: "Continue to Pay" }).click();
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.12345678");
  await expect(page.getByTestId("pay-confirm-unsigned")).toHaveCount(0);
  const calls = await page.evaluate(() => window.__dwFixture!.calls);
  expect(calls).not.toContain("draft_prepare");
  expect(calls).not.toContain("gift_card_broadcast");
}

async function send(page: Page) {
  const passphrase = page.getByTestId("pay-confirm-passphrase");
  if (await passphrase.isVisible()) await passphrase.fill("demo");
  await page.getByTestId("pay-confirm-send").click();
}

for (const theme of THEMES) {
  test(`gift cards are opt-in and use the normal Pay confirmation (${theme})`, async ({ page }) => {
    await quote(page, theme);
    await openConfirmation(page);
    await send(page);
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    await page.getByTestId("pay-result-close").click();
    await expect(page.getByText("FIXTURE-GIFT-CODE")).toBeVisible();
    await expect(page.getByText("0000", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Redeem at link-only.fixture.invalid" }).click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("gift_card_open_claim"))).toBe(true);
    await expect(page.getByRole("button", { name: "Continue to Pay" })).toHaveCount(0);
    const paid = await page.evaluate(() => window.__dwFixture!.calls);
    expect(paid.filter((c) => c === "draft_prepare")).toHaveLength(1);
    expect(paid.filter((c) => c === "gift_card_broadcast")).toHaveLength(1);
  });
}

test("returning to an order with an unknown broadcast cannot start a second payment", async ({ page }) => {
  await page.clock.install();
  await quote(page, "light");
  await openConfirmation(page);
  await page.evaluate(() => { window.__dwFixture!.world.send.broadcast = "unknown"; });
  await send(page);
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await expect(page.getByTestId("pay-result-retry")).toBeVisible();
  await page.clock.fastForward(125_000);
  await expect(page.getByTestId("pay-result-retry")).toHaveCount(0);
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await page.getByTestId("pay-result-close").click();
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("sidebar-explore").click();
  await page.getByRole("button", { name: /fixture-order-1/ }).click();
  await expect(page.getByText("A payment was started for this order. Check Activity for its status.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Continue to Pay" })).toHaveCount(0);
});

for (const network of ["testnet", "regtest"]) {
  test(`the ${network} opt-in never opens the production PiggyCards account`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", `&network=${network}`));
    await expect(page.getByTestId("sidebar-home")).toBeVisible();
    await expect(page.getByTestId("sidebar-explore")).toHaveCount(0);
    await page.getByTestId("sidebar-settings").click();
    await expect(toggle(page)).not.toBeChecked();
    await toggle(page).check();
    await page.getByTestId("sidebar-explore").click();
    await expect(page.getByText("PiggyCards gift card purchases are available on mainnet.")).toBeVisible();
    expect(await page.evaluate(() => window.__dwFixture!.calls)).not.toContain("gift_card_account");
    await page.getByTestId("sidebar-settings").click();
    await toggle(page).uncheck();
    await expect(page.getByTestId("sidebar-explore")).toHaveCount(0);
  });
}
