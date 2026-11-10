import { expect, test } from "@playwright/test";
import { fixtureUrl, openTransfer } from "./helpers";
import { hold, release } from "./transfer";

test.setTimeout(90_000);

for (const theme of ["light", "dark"] as const) {
  test(`transfer amounts and Max stay steady, ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("platform-card-transfer")).toHaveCount(0);
    await openTransfer(page, "platformToCore");
    await expect(page.getByTestId("transfer-amount")).toHaveValue("0.39998000");
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("platform-move-quote-credits")).toContainText("0.39998");
    await page.keyboard.press("Escape");
    await openTransfer(page, "coreToPlatform");
    const field = page.getByTestId("transfer-amount");
    await field.fill("0.5");
    const beforeMax = (await field.boundingBox())!;
    await hold(page, "platform_fund_quote");
    await page.getByTestId("transfer-max").click();
    await expect(field).toHaveValue("0.5");
    await expect(page.getByTestId("transfer-continue")).toBeEnabled();
    await expect(page.getByTestId("transfer-max")).toHaveAttribute("aria-busy", "true");
    await expect(page.getByTestId("transfer-fee")).not.toContainText("Working out");
    await expect(page.locator(".transfer-amount-row .dash-button-why")).toHaveCount(0);
    const duringMax = (await field.boundingBox())!;
    expect(duringMax.width).toBe(beforeMax.width);
    expect(duringMax.x).toBe(beforeMax.x);
    await release(page);
    await expect(page.getByTestId("transfer-continue")).toBeEnabled();
    await expect(page.getByTestId("transfer-to")).not.toContainText("+0.00");
  });

  test(`mixing shows spendable funds and offers pause, ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("coinjoin-mixing", theme));
    await page.getByTestId("hero").waitFor();
    await openTransfer(page, "coreToPlatform");
    const from = page.getByTestId("transfer-from");
    const mixingLine = page.getByTestId("transfer-mixing");
    await expect(mixingLine).toContainText("mixing");
    const before = await from.textContent();
    const mixingBefore = await mixingLine.textContent();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const w = f.world.wallets[0];
      w.coins[0].reserved = true;
      const denomination = w.coins.find((c) => c.coinjoinDenominated)!;
      denomination.reserved = true;
      denomination.coinjoinRounds = null;
      f.emit({ kind: "coinJoin", network: "testnet", walletId: w.summary.walletId });
    });
    await expect(from).not.toHaveText(before!);
    await expect(mixingLine).toHaveText(mixingBefore!);
    await expect(page.getByTestId("transfer-fee")).not.toHaveClass(/form-problem/);
    await expect(page.getByTestId("transfer-pause-mixing")).toBeVisible();
    await page.getByTestId("transfer-amount").fill("1000");
    await expect(page.getByTestId("transfer-fee")).toHaveText(/Not enough spendable Dash/);
    await page.getByTestId("transfer-pause-mixing").click();
    await expect(page.getByTestId("transfer-pause-mixing")).toHaveCount(0);
  });

  test(`Receive selects one QR and requests opt in individually, ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("sidebar-receive").click();
    await expect(page.getByTestId("receive-qr")).toBeVisible();
    await expect(page.getByTestId("request-include-username")).not.toBeChecked();
    await page.getByTestId("request-label").fill("Draft rent");
    await page.getByTestId("platform-receive-toggle").filter({ visible: true }).click();
    await expect(page.getByTestId("platform-receive-qr")).toBeVisible();
    await expect(page.getByTestId("request-form")).toBeHidden();
    await expect(page.locator("svg.qr:visible")).toHaveCount(1);
    await page.getByTestId("shielded-receive-open").filter({ visible: true }).click();
    await expect(page.getByTestId("shielded-receive-qr")).toBeVisible();
    await expect(page.locator("svg.qr:visible")).toHaveCount(1);
    await page.getByTestId("receive-select-dash").filter({ visible: true }).click();
    await expect(page.getByTestId("request-label")).toHaveValue("Draft rent");
    await page.getByTestId("request-include-username").check();
    await page.getByTestId("request-create").click();
    await expect(page.getByTestId("request-username")).toContainText("@alice");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("request-include-username")).not.toBeChecked();
    expect(await page.evaluate(() => window.__dwFixture!.world.wallets[0].receive.usernameInRequests)).toBe(false);
  });

  test(`compact Home keeps actions below the breakdown, ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 900, height: 900 });
    await page.goto(fixtureUrl("funded", theme));
    const breakdown = page.locator(".hero-breakdown");
    const actions = page.locator(".shortcut-card");
    await expect(breakdown).toBeVisible({ timeout: 20_000 });
    const b = (await breakdown.boundingBox())!;
    const a = (await actions.boundingBox())!;
    expect(a.y).toBeGreaterThanOrEqual(b.y + b.height);
  });

  test(`Add contact focuses and reveals its narrow pane, ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 620 });
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("sidebar-contacts").click();
    const add = page.getByTestId("contacts-add");
    await add.focus();
    await add.press("Enter");
    const search = page.getByRole("searchbox", { name: "Search username", exact: true });
    await expect(search).toBeFocused();
    await expect(search).toBeInViewport();
    await search.fill("mal");
    await page.getByTestId("add-contact-hit").click();
    await expect(page.getByTestId("add-contact-name")).toBeFocused();
    await expect(page.getByTestId("add-contact-name")).toBeInViewport();
  });

  test(`Contacts columns and header stay stable, ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 620 });
    await page.goto(fixtureUrl("dashpay-registered", theme));
    await page.getByTestId("sidebar-contacts").click();
    await expect(page.getByTestId("add-contact")).toHaveCount(0);
    await page.getByTestId("contacts-add").click();
    await expect(page.getByTestId("contacts-add")).toBeDisabled();
    const pane = page.getByTestId("contacts-pane");
    const before = (await pane.boundingBox())!;
    await page.getByRole("searchbox", { name: "Search username", exact: true }).fill("mal");
    await expect(page.getByTestId("add-contact-hit")).toBeVisible();
    const searching = (await pane.boundingBox())!;
    await page.getByTestId("add-contact-hit").click();
    const selected = (await pane.boundingBox())!;
    expect(searching.x).toBe(before.x);
    expect(searching.width).toBe(before.width);
    expect(selected.x).toBe(before.x);
    expect(selected.width).toBe(before.width);
    await expect(page.getByTestId("page-contacts").locator("h1")).toBeInViewport();
  });
}
