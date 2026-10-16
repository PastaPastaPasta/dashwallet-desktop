import { expect, test, type Page } from "@playwright/test";
import type { CommandName } from "../src/api/generated/commands";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP3-03b: Settings ▸ General ▸ Privacy, "Include my username in payment requests" (default on),
// and the Receive QR and requests with and without `user=`, on the fixture backend
// (src/fixture/send.ts). The engine's rules are dw-engine's tests (users_tests, e1_offline).

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
const box = (page: Page) => page.getByTestId("settings-username-in-requests");
const copied = (page: Page) => page.evaluate(() => (window as unknown as { __copied: string }).__copied);

/** Opens `scenario` with a clipboard the test can read. */
async function open(page: Page, scenario: string, theme: Theme) {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) },
    });
  });
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

/** Receive's address and the link its Copy link button copies (the QR's text). */
async function receiveLink(page: Page) {
  await page.getByTestId("sidebar-receive").click();
  await expect(page.getByTestId("receive-qr")).toBeVisible();
  const address = (await page.getByTestId("receive-address").textContent())!;
  await expect(page.getByTestId("receive-copy-uri")).toBeEnabled();
  await page.getByTestId("receive-copy-uri").click();
  return { address, link: await copied(page) };
}

async function generalSettings(page: Page) {
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-tabs-general").click();
  await expect(page.getByTestId("settings-privacy")).toBeVisible();
}

const optedIn = (page: Page) => page.evaluate(() => window.__dwFixture!.world.wallets[0].receive.usernameInRequests);

for (const theme of THEMES)
  test(`on, the Receive QR and a request carry user=; off, they and the stored request lose it, ${theme}`, async ({ page }) => {
    // Three axe passes and two Receive visits: WebKit under load needs more than 30 s.
    test.setTimeout(60_000);
    const errors = await open(page, "dashpay-registered", theme);
    const on = await receiveLink(page);
    expect(on.link).toBe(`dash:${on.address}?user=alice`);
    await page.getByTestId("request-label").fill("Rent");
    await page.getByTestId("request-create").click();
    await expect(page.getByTestId("request-uri")).toContainText("?label=Rent&user=alice");
    await page.keyboard.press("Escape");
    expect(await seriousAxeViolations(page)).toEqual([]);

    await generalSettings(page);
    await expect(box(page)).toBeChecked();
    await expect(box(page)).toHaveAccessibleDescription(/“Main” show its DashPay username/);
    await box(page).uncheck();
    await expect(box(page)).not.toBeChecked();
    await expect(box(page)).toBeEnabled();
    expect(await optedIn(page)).toBe(false);
    expect(await seriousAxeViolations(page)).toEqual([]);

    // Off: the QR is dash-qt's URI, and the stored request follows (DP3-02 D18).
    const { address, link } = await receiveLink(page);
    expect(link).toBe(`dash:${address}`);
    await page.getByTestId("requests-table").locator('[data-testid^="request-show-"]').click();
    await expect(page.getByTestId("request-uri")).toContainText("?label=Rent");
    await expect(page.getByTestId("request-uri")).not.toContainText("user=");
    await expect(page.getByTestId("request-username")).toHaveCount(0);
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

test("a wallet with the toggle off shows plain Core links until it is turned on", async ({ page }) => {
  const errors = await open(page, "dashpay-username-off", "light");
  const { address, link } = await receiveLink(page);
  expect(link).toBe(`dash:${address}`);
  await generalSettings(page);
  await expect(box(page)).not.toBeChecked();
  await box(page).check();
  await expect(box(page)).toBeEnabled();
  expect(await optedIn(page)).toBe(true);
  expect((await receiveLink(page)).link).toBe(`dash:${address}?user=alice`);
  expect(errors).toEqual([]);
});

test("back on Receive after turning it off, no cached name shows while the username is read again", async ({ page }) => {
  const errors = await open(page, "dashpay-registered", "light");
  const { address, link } = await receiveLink(page);
  expect(link).toBe(`dash:${address}?user=alice`);
  await generalSettings(page);
  await box(page).uncheck();
  await expect(box(page)).toBeEnabled();
  // The engine is slow to answer Receive's reads: what shows meanwhile comes from the cache.
  const held: CommandName = "receive_username";
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), held);
  expect((await receiveLink(page)).link).toBe(`dash:${address}`);
  await release.evaluate((r) => r());
  expect(errors).toEqual([]);
});

test("leaving Settings before the engine answers, Receive already shows no name (D7)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered", "light");
  const { address } = await receiveLink(page);
  await generalSettings(page);
  const held: CommandName = "set_username_in_requests";
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), held);
  await box(page).uncheck();
  await expect(box(page)).toBeDisabled();
  expect((await receiveLink(page)).link).toBe(`dash:${address}`);
  await release.evaluate((r) => r());
  expect(await optedIn(page)).toBe(false);
  expect(errors).toEqual([]);
});

test("an unreadable setting shows off with the problem, and a click writes it again (D6)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered", "light");
  await page.evaluate(() => window.__dwFixture!.fail("username_in_requests", "none"));
  await generalSettings(page);
  await expect(page.getByTestId("settings-username-error")).toBeVisible();
  await expect(box(page)).not.toBeChecked();
  await page.evaluate(() => window.__dwFixture!.fail("username_in_requests", null));
  await box(page).check();
  await expect(box(page)).toBeChecked();
  await expect(box(page)).toBeEnabled();
  await expect(page.getByTestId("settings-username-error")).toHaveCount(0);
  expect(await optedIn(page)).toBe(true);
  expect(errors).toEqual([]);
});

test("Receive drops the name it holds once the toggle reads off, never showing it meanwhile (DP3-02 D25, D18)", async ({ page }) => {
  const errors = await open(page, "dashpay-registered", "light");
  const { address, link } = await receiveLink(page);
  expect(link).toBe(`dash:${address}?user=alice`);
  // Turned off elsewhere while the page is open, then a names change re-reads the receive side.
  // While the plain link is being built, the named one is not kept in its place (D18).
  const held: CommandName = "build_payment_uri";
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), held);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.wallets[0].receive.usernameInRequests = false;
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "names" } });
  });
  await expect(page.getByTestId("receive-copy-uri")).toBeDisabled();
  await release.evaluate((r) => r());
  await expect(async () => {
    await page.getByTestId("receive-copy-uri").click();
    expect(await copied(page)).toBe(`dash:${address}`);
  }).toPass();
  expect(errors).toEqual([]);
});

test("the box shows the choice until the engine answers, and a refusal keeps the setting", async ({ page }) => {
  const errors = await open(page, "dashpay-registered", "light");
  await generalSettings(page);
  const held: CommandName = "set_username_in_requests";
  const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), held);
  await box(page).uncheck();
  await expect(box(page)).not.toBeChecked();
  await expect(box(page)).toBeDisabled();
  await release.evaluate((r) => r());
  await expect(box(page)).toBeEnabled();
  await expect(box(page)).not.toBeChecked();

  // The box reverts once the refusal is in, so Playwright's check() would not see it change.
  await page.evaluate(() => window.__dwFixture!.fail("set_username_in_requests", "none"));
  await box(page).click();
  await expect(page.getByTestId("settings-username-problem")).toBeVisible();
  await expect(box(page)).not.toBeChecked();
  expect(await optedIn(page)).toBe(false);
  expect(errors).toEqual([]);
});
