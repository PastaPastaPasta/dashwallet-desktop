import { expect, test } from "@playwright/test";
import { authorize, fixtureUrl, openTransfer, THEMES } from "./helpers";

for (const theme of THEMES) {
  test(`Home has no pool cards in the normal state (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    for (const id of ["shielded-card", "platform-card", "shielded-card-text", "platform-card-amount", "shielded-card-transfer", "platform-card-transfer"]) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shortcut-transfer").click();
    await expect(page.getByTestId("transfer-sheet")).toBeVisible();
  });

  test(`Home attention sections retain both Finish paths (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("paddr-fund-proof-waiting", theme));
    await expect(page.getByTestId("platform-fund-unfinished")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }];
      f.emit({ kind: "eventsOverflowed" });
    });
    await expect(page.getByTestId("shielded-unfinished")).toBeVisible();
    await expect(page.getByTestId("shielded-card").getByRole("heading")).toHaveText("Shielded");
    await expect(page.getByTestId("platform-card").getByRole("heading")).toHaveText("Platform addresses");
    for (const id of ["shielded-card-text", "platform-card-amount", "shielded-card-transfer", "platform-card-transfer"]) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-unfinished")).toHaveCount(0);
    await expect(page.getByTestId("shielded-card")).toHaveCount(0);
    await page.getByTestId("platform-fund-finish").click();
    await authorize(page);
    await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
    await page.getByTestId("platform-fund-done").click();
    await expect(page.getByTestId("platform-fund-unfinished")).toHaveCount(0);
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
  });

  for (const pool of ["shielded", "platform"] as const) {
    test(`Home retains an unseen ${pool} result and Show (${theme})`, async ({ page }) => {
      const command = pool === "shielded" ? "shield_from_core" : "platform_fund";
      const prefix = pool === "shielded" ? "shielded" : "platform-fund";
      await page.goto(fixtureUrl("funded", theme, `&hold=${command}`));
      await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
      await openTransfer(page, pool === "shielded" ? "coreToShielded" : "coreToPlatform");
      await page.getByTestId("transfer-amount").fill("0.001");
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId(`${prefix}-go`).click();
      await authorize(page);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      await page.evaluate((command) => window.__dwFixture!.release(command as "shield_from_core" | "platform_fund"), command);
      await expect(page.getByTestId(`${prefix}-unseen`)).toBeVisible();
      await expect(page.getByTestId("shielded-card-transfer")).toHaveCount(0);
      await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
      await page.getByTestId(`${prefix}-show`).click();
      await expect(page.getByTestId(`${prefix}-outcome-title`)).toHaveText(pool === "shielded" ? "Added to Shielded" : "Added to Platform");
    });
  }
}

test("An open Shielded passphrase prompt survives the Home section appearing and going", async ({ page }) => {
  await page.goto(fixtureUrl("funded", THEMES[0]));
  await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
  await expect(page.getByTestId("shielded-card")).toHaveCount(0);
  await openTransfer(page, "coreToShielded");
  await page.getByTestId("transfer-amount").fill("0.001");
  await page.getByTestId("transfer-continue").click();
  await page.getByTestId("shielded-go").click();
  const input = page.getByTestId("grant-sheet").locator("input[type=password]");
  await input.fill("de");
  await input.evaluate((el) => el.setAttribute("data-kept", "1"));
  const setShields = (n: number) =>
    page.evaluate((n) => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = n ? [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }] : [];
      f.emit({ kind: "eventsOverflowed" });
    }, n);
  await setShields(1);
  await expect(page.getByTestId("shielded-unfinished")).toHaveCount(1);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await setShields(0);
  await expect(page.getByTestId("shielded-card")).toHaveCount(0);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await input.pressSequentially("mo");
  await input.press("Enter");
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
});
