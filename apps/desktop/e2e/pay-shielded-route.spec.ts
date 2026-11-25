import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl } from "./helpers";

const SHIELDED_ADDRESS = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";

async function payTo(page: Page, scenario: string, address: string, amount = "") {
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("hero-amount").waitFor();
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  if (amount) await page.getByTestId("pay-amount").fill(amount);
  await page.getByTestId("pay-address").fill(address);
}

test.describe("a Shielded address in Pay to", () => {
  test("points to Send from Shielded, which opens with the address and amount", async ({ page }) => {
    await payTo(page, "funded", SHIELDED_ADDRESS, "0.2");
    const field = page.getByTestId("pay-address-form").locator(".form-field").first();
    await expect(field.locator(".field-hint")).toHaveText("This is a shielded address. Pay it from your Shielded balance with Send from Shielded.");
    await expect(field.locator(".field-error")).toHaveCount(0);
    await expect(page.getByTestId("pay-address")).toHaveAttribute("aria-invalid", "false");
    await expect(page.getByText(/cannot pay shielded addresses/)).toHaveCount(0);
    await page.getByTestId("pay-shielded-route").click();
    await expect(page.getByTestId("shielded-to")).toHaveValue(SHIELDED_ADDRESS);
    await expect(page.getByTestId("shielded-amount")).toHaveValue("0.2");
  });

  test("as an added recipient, says to pay it on its own with Send from Shielded", async ({ page }) => {
    await payTo(page, "funded", "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf");
    await page.getByTestId("pay-add-recipient").click();
    await page.getByTestId("pay-more-2-address").fill(SHIELDED_ADDRESS);
    await expect(page.getByTestId("pay-more-2").locator(".field-error").first()).toHaveText("A shielded address can't be one of several recipients. Pay it on its own with Send from Shielded.");
  });

  test("still says it cannot be paid when Shielded is not on", async ({ page }) => {
    await payTo(page, "shielded-inactive", SHIELDED_ADDRESS);
    const field = page.getByTestId("pay-address-form").locator(".form-field").first();
    await expect(field.locator(".field-error")).toHaveText("This is a shielded address. This wallet cannot pay shielded addresses yet.");
    await expect(page.getByTestId("pay-shielded-route")).toHaveCount(0);
  });

  test("waits, and says why, while Shielded syncs", async ({ page }) => {
    await payTo(page, "shielded-syncing", SHIELDED_ADDRESS);
    const button = page.getByTestId("pay-shielded-route");
    await expect(button).toBeDisabled();
    const id = await button.getAttribute("aria-describedby");
    expect(id).toBeTruthy();
    await expect(page.locator(`[id="${id}"]`)).toHaveText("Shielded is still syncing. Try again when it finishes.");
  });

  // MG: when the vault's lock or a watch-only wallet holds the account, the line names it.
  for (const [scenario, line] of [
    ["shielded-locked", "Shielded starts syncing when you unlock the wallet. Unlock it, then try again once the sync finishes."],
    ["shielded-mixing-only", "The wallet is unlocked for mixing only, so Shielded can't sync. Unlock it fully, then try again once the sync finishes."],
    ["shielded-watch-only", "Shielded isn't available in a watch-only wallet."],
  ])
    test(`waits, and names the cause, in ${scenario}`, async ({ page }) => {
      await payTo(page, scenario, SHIELDED_ADDRESS);
      const button = page.getByTestId("pay-shielded-route");
      await expect(button).toBeDisabled();
      const id = await button.getAttribute("aria-describedby");
      await expect(page.locator(`[id="${id}"]`)).toHaveText(line);
    });
});
