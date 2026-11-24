import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, openTransfer } from "./helpers";

const SYNCING = "Shielded is still syncing; try again when it finishes.";

/** The visible text a disabled control is described by. */
async function reasonOf(page: Page, testId: string) {
  const button = page.getByTestId(testId);
  await expect(button).toBeDisabled();
  const id = await button.getAttribute("aria-describedby");
  expect(id, `${testId} is described`).toBeTruthy();
  const text = page.locator(`[id="${id}"]`);
  await expect(text).toBeVisible();
  return (await text.textContent())!.trim();
}

test.describe("Transfer to Shielded while the wallet's Core sync runs", () => {
  test("says it waits on the form, before any quote or passphrase", async ({ page }) => {
    await page.goto(fixtureUrl("shielded-core-syncing", "light"));
    await page.getByTestId("hero-amount").waitFor();
    await openTransfer(page, "coreToShielded");
    await expect(page.getByTestId("transfer-unavailable")).toHaveText(SYNCING);
    await expect(page.getByTestId("transfer-amount")).toBeDisabled();
    for (const id of ["transfer-max", "transfer-continue"]) expect(await reasonOf(page, id)).toContain(SYNCING);
    await expect(page.getByTestId("shielded-quote-total")).toHaveCount(0);
  });

  test("moving out of Shielded does not wait for it", async ({ page }) => {
    await page.goto(fixtureUrl("shielded-core-syncing", "light"));
    await page.getByTestId("hero-amount").waitFor();
    await openTransfer(page, "shieldedToCore");
    await page.getByTestId("transfer-amount").fill("0.5");
    await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  });
});
