import { expect, test, type Locator, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, fontsReady, openToolsTab, THEMES, trackPageErrors, type Theme } from "./helpers";

// Layout stability guards: nearby controls keep their bounding box when results, errors and loading
// states change across Tools, Settings and Receive.

const COMPLETE = Buffer.from("psbt\xffcomplete", "latin1").toString("base64");

async function open(page: Page, scenario = "funded", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const boxesOf = (controls: Locator[]) => Promise.all(controls.map((c) => c.boundingBox()));

test.describe("Layout stability across results and loading states", () => {
  test("Tools ▸ Sign/Verify keeps action buttons and cards in place across outcomes", async ({ page }) => {
    const errors = await open(page);
    await openToolsTab(page, "signVerify");
    await fontsReady(page);

    const signControls = [page.getByTestId("sign-sign"), page.getByTestId("sign-clear"), page.getByTestId("verify-card")];
    const beforeSign = await boxesOf(signControls);

    await page.getByTestId("sign-address").fill("nonsense");
    await page.getByTestId("sign-sign").click();
    await expect(page.getByTestId("sign-result")).toContainText("The entered address is invalid.");
    expect(await boxesOf(signControls)).toEqual(beforeSign);

    await page.getByTestId("sign-clear").click();
    await expect(page.getByTestId("sign-result")).toBeEmpty();
    expect(await boxesOf(signControls)).toEqual(beforeSign);

    // Valid sign keeps Sign and Clear buttons in place even when the signature output appears below them.
    const signButtons = [page.getByTestId("sign-sign"), page.getByTestId("sign-clear")];
    const beforeValidSign = await boxesOf(signButtons);
    await page.getByTestId("sign-choose").click();
    await page.locator("[data-testid^=address-book-choose-]").first().click();
    await page.getByTestId("sign-message").fill("hello dash");
    await page.getByTestId("sign-sign").click();
    await expect(page.getByTestId("sign-result")).toHaveText("Message signed.");
    await expect(page.getByTestId("sign-signature")).toBeVisible();
    expect(await boxesOf(signButtons)).toEqual(beforeValidSign);

    // Verify card buttons stay in place across invalid, valid, and cleared outcomes.
    await page.getByTestId("verify-card").scrollIntoViewIfNeeded();
    const verifyButtons = [page.getByTestId("verify-verify"), page.getByTestId("verify-clear")];
    const beforeVerify = await boxesOf(verifyButtons);
    await page.getByTestId("verify-address").fill("nonsense");
    await page.getByTestId("verify-signature").fill("nonsense");
    await page.getByTestId("verify-verify").click();
    await expect(page.getByTestId("verify-result")).toHaveText("Invalid address: check it and try again.");
    expect(await boxesOf(verifyButtons)).toEqual(beforeVerify);

    await page.getByTestId("verify-clear").click();
    await expect(page.getByTestId("verify-result")).toBeEmpty();
    expect(await boxesOf(verifyButtons)).toEqual(beforeVerify);

    expect(errors).toEqual([]);
  });

  test("Tools ▸ Repair keeps all cards in place when an outcome appears", async ({ page }) => {
    const errors = await open(page);
    await openToolsTab(page, "repair");
    await fontsReady(page);

    const cards = [
      page.getByTestId("repair-rescan"),
      page.getByTestId("repair-birth"),
      page.getByTestId("repair-actions"),
      page.getByTestId("repair-finish"),
    ];
    const before = await boxesOf(cards);

    await page.getByTestId("repair-rescan-height").fill("12.5");
    await page.getByTestId("repair-rescan-from").click();
    await expect(page.getByTestId("repair-outcome")).toHaveText("Enter a block height.");
    expect(await boxesOf(cards)).toEqual(before);

    expect(errors).toEqual([]);
  });

  test("Tools ▸ PSBT keeps cards and operation buttons in place across errors and results", async ({ page }) => {
    const errors = await open(page);
    await openToolsTab(page, "psbt");
    await fontsReady(page);

    const externalCard = [page.getByTestId("psbt-external-signer-not-yet")];
    const beforeError = await boxesOf(externalCard);
    await page.getByTestId("psbt-paste").fill("not base64!");
    await page.getByTestId("psbt-load-text").click();
    await expect(page.getByTestId("psbt-error")).toHaveText("Unable to decode PSBT (invalid base64)");
    expect(await boxesOf(externalCard)).toEqual(beforeError);

    await page.getByTestId("psbt-paste").fill(COMPLETE);
    await page.getByTestId("psbt-load-text").click();
    await expect(page.getByTestId("psbt-operations")).toBeVisible();
    await fontsReady(page);

    const opButtons = [
      page.getByTestId("psbt-sign"),
      page.getByTestId("psbt-broadcast"),
      page.getByTestId("psbt-copy"),
      page.getByTestId("psbt-save"),
    ];
    const beforeBroadcast = await boxesOf(opButtons);
    await page.getByTestId("psbt-broadcast").click();
    await expect(page.getByTestId("psbt-message")).toContainText("Transaction broadcast successfully! Transaction ID:");
    expect(await boxesOf(opButtons)).toEqual(beforeBroadcast);

    expect(errors).toEqual([]);
  });

  test("Tools ▸ Information keeps DashPay sync button and detail rows in place after Sync now", async ({ page }) => {
    const errors = await open(page, "dashpay-info-spv");
    await openToolsTab(page, "information");
    await expect(page.getByTestId("information-dashpay-rows")).toBeVisible();
    await page.getByTestId("information-dashpay").scrollIntoViewIfNeeded();
    await fontsReady(page);

    const controls = [page.getByTestId("information-dashpay-sync-now"), page.getByTestId("information-dashpay-rows")];
    const before = await boxesOf(controls);

    await page.getByTestId("information-dashpay-sync-now").click();
    await expect(page.getByTestId("information-dashpay-outcome")).toContainText("A DashPay pass was asked for.");
    expect(await boxesOf(controls)).toEqual(before);

    expect(errors).toEqual([]);
  });

  test("Settings ▸ About ▸ Updates keeps controls and following cards in place across check outcomes", async ({ page }) => {
    const errors = await open(page);
    await page.evaluate(() => {
      window.__dwFixture!.world.updates.availability = "available";
    });
    await page.getByTestId("sidebar-settings").click();
    await page.getByTestId("settings-tabs-about").click();
    await expect(page.getByTestId("settings-updates-check")).toBeVisible();
    await page.getByTestId("settings-updates").scrollIntoViewIfNeeded();
    await fontsReady(page);

    const controls = [
      page.getByTestId("settings-updates-auto"),
      page.getByTestId("settings-updates-check"),
      page.getByTestId("settings-about-more"),
      page.getByTestId("settings-about-links"),
    ];
    const before = await boxesOf(controls);

    await page.getByTestId("settings-updates-check").click();
    await expect(page.getByTestId("settings-updates-upToDate")).toBeVisible();
    expect(await boxesOf(controls)).toEqual(before);

    // A failed check keeps the controls and cards below in place as well.
    await page.evaluate(() => {
      window.__dwFixture!.world.updates.fault = "offline";
    });
    await page.getByTestId("settings-updates-check").click();
    await expect(page.getByTestId("settings-updates-problem")).toBeVisible();
    expect(await boxesOf(controls)).toEqual(before);

    expect(errors).toEqual([]);
  });

  for (const theme of THEMES) {
    test(`Receive Dash loading placeholder matches the QR slot size (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      const release = await page.evaluateHandle(() => window.__dwFixture!.hold("current_receive_address"));
      await page.getByTestId("sidebar-receive").click();
      const loading = page.getByTestId("receive-loading");
      await expect(loading).toBeVisible();
      await expect(loading.locator(".qr-placeholder-label")).toBeVisible();
      await expect(loading).toContainText("Loading address");
      const loadingBox = await loading.boundingBox();
      expect(loadingBox?.width).toBe(200);
      expect(loadingBox?.height).toBe(200);
      await release.evaluate((r) => r());
      await expect(page.getByTestId("receive-qr")).toBeVisible();
      const qrBox = await page.getByTestId("receive-qr").boundingBox();
      expect(qrBox?.width).toBe(200);
      expect(qrBox?.height).toBe(200);
      expect(errors).toEqual([]);
    });
  }
});
