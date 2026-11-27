import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, openTransfer, trackPageErrors } from "./helpers";
import { callsOf } from "./transfer";

// The Platform addresses' funds paths on mainnet, where this version keeps them off (the engine's
// flag `platform_address_sends`): the plain line in place of each control, the held line while the
// addresses hold Dash, and a held move's exit. Each state is shot in light and dark.

const PAYEE = "dash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const OFF = "This version can move and send from Platform addresses on test networks only.";
const HELD = /^[\d.]+ DASH is on your Platform addresses\. It's still yours\. This version can move it to your Dash wallet on test networks only\.$/;
const EXIT =
  "A payment or move you made earlier from your Platform balance hasn't finished, and Dash Wallet can't tell yet whether it will go through. You can move what's on those Platform addresses to your Dash wallet instead. Only one of the two can go through, never both. If the earlier one goes through first, its recipient is paid and this move fails. If this move goes through, the earlier one doesn't, so whatever it paid for stays unpaid.";

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, "&network=mainnet"));
  await page.getByTestId("hero").waitFor();
  return errors;
}

async function shot(page: Page, name: string) {
  await fontsReady(page);
  await page.screenshot({ path: test.info().outputPath(`${name}.png`), animations: "disabled", caret: "hide" });
}

async function done(page: Page, errors: string[]) {
  expect(await seriousAxeViolations(page)).toEqual([]);
  expect(errors).toEqual([]);
}

for (const theme of ["light", "dark"] as const) {
  test.describe(`Platform addresses on mainnet (${theme})`, () => {
    test("Platform → Dash wallet says the balance is still there, and offers no Move", async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-mainnet-off")).toHaveText(HELD);
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      await expect(page.getByTestId("transfer-amount")).toHaveCount(0);
      await page.getByTestId("transfer-to").click();
      await expect(page.getByTestId("transfer-picker-platform-chip")).toHaveText("Test networks only");
      await page.getByTestId("transfer-to").click();
      await shot(page, `${theme}-transfer-held-line`);
      expect(await callsOf(page, "platform_move_to_core_quote")).toBe(0);
      await done(page, errors);
    });

    test("an empty balance gets the plain line", async ({ page }) => {
      const errors = await open(page, "platform-empty", theme);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-mainnet-off")).toHaveText(OFF);
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      await shot(page, `${theme}-transfer-off`);
      await done(page, errors);
    });

    test("Dash wallet → Platform is off, with no form", async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await openTransfer(page, "platformToCore");
      await page.getByTestId("transfer-from").click();
      await page.getByTestId("transfer-picker-core").click();
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-platform").click();
      await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "unsupported");
      await expect(page.getByTestId("transfer-unavailable")).toHaveText("This version can add Dash to Platform on test networks only.");
      await expect(page.getByTestId("transfer-continue")).toHaveCount(0);
      await shot(page, `${theme}-transfer-add-off`);
      expect(await callsOf(page, "platform_fund_quote")).toBe(0);
      await done(page, errors);
    });

    test("a held move is offered as its exit, and the screen falls back once it settles", async ({ page }) => {
      const errors = await open(page, "platform-move-failed-may-land", theme);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-mainnet-exit")).toHaveText(EXIT);
      await expect(page.getByTestId("transfer-mainnet-rest")).toHaveCount(0);
      await expect(page.getByTestId("transfer-continue")).toHaveText("Move to Dash wallet");
      await expect(page.getByTestId("transfer-continue")).toBeEnabled();
      await shot(page, `${theme}-transfer-held-exit`);
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId("platform-move-go").click();
      await authorize(page);
      await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
      expect(await callsOf(page, "platform_move_to_core")).toBe(1);
      await done(page, errors);
    });

    test("a held move's details offer no Cancel: it waits for Platform", async ({ page }) => {
      const errors = await open(page, "platform-move-failed-may-land", theme);
      await page.getByTestId("sidebar-activity").click();
      await page.getByTestId("tx-list").waitFor();
      await page.locator('button[data-txid="platform:moveToCore:pm-held:mayland"]').click();
      await page.getByTestId("source-detail").waitFor();
      await expect(page.getByTestId("source-detail-status")).toHaveText("May still go through");
      await expect(page.getByTestId("source-detail-cancel")).toHaveCount(0);
      await shot(page, `${theme}-held-move-no-cancel`);
      expect(await callsOf(page, "platform_cancel_quote")).toBe(0);
      await done(page, errors);
    });

    test("a held move with more on the addresses names the rest", async ({ page }) => {
      const errors = await open(page, "platform-move-held-and-rest", theme);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-mainnet-rest")).toHaveText("The rest of your Platform balance, 0.10000000 DASH, is still yours. This version can move it on test networks only.");
      await expect(page.getByTestId("transfer-mainnet-exit")).toHaveText(`${EXIT} The rest of your Platform balance, 0.10000000 DASH, is still yours. This version can move it on test networks only.`);
      // The exit moves the held move's inputs only: the rest stays.
      await expect(page.getByTestId("transfer-amount")).toHaveValue(/^0\.39/);
      await shot(page, `${theme}-transfer-held-exit-rest`);
      await done(page, errors);
    });

    test("one hold of several records is one exit, counted, with one button", async ({ page }) => {
      const errors = await open(page, "platform-move-held-several", theme);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-mainnet-exit")).toHaveText(/^2 payments or moves you made earlier from your Platform balance haven't finished, /);
      await expect(page.getByTestId("transfer-continue")).toHaveText("Move to Dash wallet");
      await expect(page.getByRole("button", { name: "Move to Dash wallet" })).toHaveCount(1);
      await shot(page, `${theme}-transfer-held-exit-several`);
      await done(page, errors);
    });

    test("Receive ▸ Platform shows the line, with no address, QR code or Copy", async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await page.getByTestId("sidebar-receive").click();
      await page.getByTestId("receive-qr").waitFor();
      await page.getByTestId("platform-receive-toggle").click();
      await expect(page.getByTestId("platform-receive-mainnet-off")).toHaveText("This version can receive to a Platform address on test networks only. To receive Dash, use your Dash wallet address.");
      await expect(page.getByTestId("platform-receive-address")).toHaveCount(0);
      await expect(page.getByTestId("platform-receive-qr")).toHaveCount(0);
      await expect(page.getByTestId("platform-receive-copy")).toHaveCount(0);
      await shot(page, `${theme}-receive-off`);
      expect(await callsOf(page, "platform_receive_address")).toBe(0);
      await done(page, errors);
    });

    test("Pay to a Platform address shows the line, with no amount or Review", async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("pay-tabs-address").click();
      await page.getByTestId("pay-address").fill(PAYEE);
      await expect(page.getByTestId("platform-send-mainnet-off")).toHaveText(OFF);
      await expect(page.getByTestId("pay-amount")).toHaveCount(0);
      await expect(page.getByTestId("pay-continue")).toHaveCount(0);
      await shot(page, `${theme}-pay-off`);
      expect(await callsOf(page, "platform_send_quote")).toBe(0);
      await done(page, errors);
    });
  });
}
