import { expect, test } from "@playwright/test";
import { XSS_PAYLOADS } from "../src/fixture/scenarios";

// U6: attacker-controlled text (labels, names) renders as text and never runs.
test("XSS payloads in a label, a contact name and the wallet name render as text", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto("/?scenario=xss");
  await page.getByTestId("hero").waitFor();
  await expect(page.getByTestId("toolbar")).toContainText(XSS_PAYLOADS.walletName);
  await expect(page.locator(".tx-row-title").first()).toHaveText(XSS_PAYLOADS.label);
  await expect(page.locator(".tx-row-title", { hasText: XSS_PAYLOADS.contact })).toHaveCount(1);
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => (window as unknown as { __xss?: string }).__xss)).toBeUndefined();
  expect(await page.locator('img[src="x"], script:not([type="module"]), svg[onload]').count()).toBe(0);
  expect(errors).toEqual([]);
});
