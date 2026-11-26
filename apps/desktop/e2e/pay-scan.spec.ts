import { expect, test, type Page } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Pay ▸ To an address ▸ Scan QR (IOS-043 on a desktop without a camera): Open Image…, a pasted or
// dropped image, and a screen region (the fixture's picker; nothing is captured on the host). A
// code that pays fills the form in place; a bare Core or Platform address fills Pay to.

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const PLATFORM = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";
const qrImage = (...codes: string[]) => ["fixture-qr:", ...codes].join("\n");

async function openPay(page: Page, theme: Theme = "light") {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-address").waitFor();
}

const pasteImage = (page: Page, codes: string[]) =>
  page.getByTestId("pay-address").evaluate((el, c) => {
    const dt = new DataTransfer();
    dt.items.add(new File([["fixture-qr:", ...c].join("\n")], "pasted.png", { type: "image/png" }));
    el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }));
  }, codes);

const dropImage = (page: Page, content: string) =>
  page.getByTestId("pay-address").evaluate((el, c) => {
    const dt = new DataTransfer();
    dt.items.add(new File([c], "code.png", { type: "image/png" }));
    for (const type of ["dragenter", "dragover", "drop"]) el.dispatchEvent(new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, content);

test("IOS-043 Scan QR on Pay: an image's payment link prefills Pay; a bare address fills Pay to", async ({ page }) => {
  const errors = trackPageErrors(page);
  await openPay(page);
  await page.evaluate((img) => (window.__dwFixture!.world.tools.files.open = { name: "qr.png", data: btoa(img) }), qrImage(`dash:${ADDRESS}?amount=0.25&label=Rent`));
  await page.getByTestId("pay-scan-image").click();
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
  expect(await page.evaluate(() => window.__dwFixture!.world.tools.files.dialogs.map((d) => d.purpose))).toEqual(["qrImage"]);
  // A typed message stays when the link carries none; the link's label and amount fill theirs.
  await page.getByTestId("pay-message").fill("my note");
  await page.getByTestId("pay-scan-image").click();
  await expect(page.getByTestId("pay-message")).toHaveValue("my note");
  // Scanning only fills the form: the amount stays editable, and nothing goes without Confirm.
  const amount = page.getByTestId("pay-amount");
  await expect(amount).toHaveValue("0.25000000");
  await amount.fill("0.3");
  const sent = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "draft_prepare" || c === "draft_broadcast").length);
  const before = await sent();
  await page.getByTestId("pay-continue").click();
  await expect(page.getByTestId("pay-confirm-address")).toContainText(ADDRESS);
  await expect(page.getByTestId("pay-confirm-amount")).toContainText("0.3");
  expect(await sent()).toBe(before);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("pay-confirm")).toHaveCount(0);

  await page.getByTestId("pay-address").fill("");
  await pasteImage(page, ["not a code", ADDRESS]);
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);

  // The wallet's own Platform receive QR is a bare Platform address: it fills Pay to too.
  await pasteImage(page, [PLATFORM]);
  await expect(page.getByTestId("pay-address")).toHaveValue(PLATFORM);
  await page.getByTestId("pay-address").fill("");
  // An invitation code is skipped, not saved in the link stash, and the payment after it is taken.
  const routed = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "route_link").length);
  const routedBefore = await routed();
  await pasteImage(page, ["dashpay://invite?du=alice&assetlocktx=00&pk=00&islock=00", ADDRESS]);
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
  expect(await routed()).toBe(routedBefore);

  await dropImage(page, "GIF89a");
  await expect(page.getByTestId("pay-scan-error")).toHaveText("This image could not be read. Use a PNG, JPEG or BMP file.");
  await dropImage(page, qrImage("dashpay://user?id=xyz&username=bob"));
  await expect(page.getByTestId("pay-scan-error")).toHaveText("There is no Dash address or payment link in this QR code.");
  await dropImage(page, qrImage("dash:?r=https://merchant.example/pay"));
  await expect(page.getByTestId("pay-scan-error")).not.toHaveText("There is no Dash address or payment link in this QR code.");
  expect(errors).toEqual([]);
});

test("IOS-043 Scan a Screen Region on Pay: the picker's region, and Cancel", async ({ page }) => {
  await openPay(page);
  await page.evaluate((img) => (window.__dwFixture!.world.scan.screen = btoa(img)), qrImage(`dash:${ADDRESS}`));
  await page.getByTestId("pay-scan-screen").click();
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
  await page.evaluate(() => (window.__dwFixture!.world.scan.holdScreen = true));
  await page.getByTestId("pay-scan-screen").click();
  await expect(page.getByTestId("pay-scan-status")).toHaveText("Select a region of the screen…");
  await page.getByTestId("pay-scan-screen-cancel").click();
  await expect(page.getByTestId("pay-scan-status")).toHaveCount(0);
  expect(await page.evaluate(() => window.__dwFixture!.world.scan.waiting.length)).toBe(0);
  // Editing the address drops a capture still waiting: its late result fills nothing.
  await page.evaluate((img) => (window.__dwFixture!.world.scan.screen = btoa(img)), qrImage(`dash:${PLATFORM}`));
  await page.getByTestId("pay-scan-screen").click();
  await page.getByTestId("pay-address").fill("");
  await expect(page.getByTestId("pay-scan-status")).toHaveCount(0);
  await page.evaluate(() => {
    const scan = window.__dwFixture!.world.scan;
    scan.holdScreen = false;
    for (const done of scan.waiting.splice(0)) done();
  });
  await expect(page.getByTestId("pay-address")).toHaveValue("");
});

for (const theme of THEMES)
  test(`Scan QR on Pay, ${theme}`, async ({ page }, info) => {
    await openPay(page, theme);
    await fontsReady(page);
    await page.getByTestId("pay-address-form").screenshot({ path: artifactPath(info, `pay-scan-${theme}.png`) });
  });
