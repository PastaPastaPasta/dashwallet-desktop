import { expect, test } from "@playwright/test";
import { RELEASES_URL } from "../src/protocol/advisory";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

for (const theme of THEMES) {
  for (const scenario of ["protocol-unknown", "protocol-equal"] as const) {
    test(`${scenario} (${theme}): shows no advisory banner`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl(scenario, theme));
      await expect(page.getByTestId("hero-amount")).toBeVisible();
      await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("protocol_compatibility"))).toBe(true);
      await expect(page.getByTestId("protocol-advisory-banner")).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

  test(`protocol-upgraded (${theme}): shows non-blocking banner, opens releases, axe clean, dismisses for session`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("protocol-upgraded", theme));
    const banner = page.getByTestId("protocol-advisory-banner");
    await expect(banner).toBeVisible();
    await fontsReady(page);

    await expect(page.getByTestId("protocol-advisory-text")).toHaveText("The network has upgraded. Check for an app update.");
    const link = page.getByTestId("protocol-advisory-link");
    await expect(link).toHaveAttribute("href", RELEASES_URL);
    expect(await seriousAxeViolations(page)).toEqual([]);

    await link.click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.releasesOpened ?? 0)).toBe(1);
    await expect(page.getByTestId("page-home")).toBeVisible();

    // Non-blocking: wallet actions remain usable while the advisory is shown.
    await expect(page.getByTestId("shortcut-receive")).toBeEnabled();
    await expect(page.getByTestId("shortcut-send")).toBeEnabled();
    await expect(page.getByTestId("shortcut-transfer")).toBeEnabled();
    await page.getByTestId("shortcut-receive").click();
    await expect(page.getByTestId("receive-qr")).toBeVisible();
    await page.getByTestId("sidebar-home").click();
    await expect(banner).toBeVisible();

    // Dismissing hides the banner across route changes for this session, and reload restores it.
    await page.getByTestId("protocol-advisory-dismiss").click();
    await expect(banner).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    await expect(page.getByTestId("tx-list")).toBeVisible();
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("hero-amount")).toBeVisible();
    await expect(banner).toHaveCount(0);

    await page.reload();
    await expect(page.getByTestId("protocol-advisory-banner")).toBeVisible();
    expect(errors).toEqual([]);
  });
}

test("banner appears live when Platform status learns an upgraded version, and surfaces open errors", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("protocol-unknown", "light", "&fail=open_releases_page:none"));
  await expect(page.getByTestId("hero-amount")).toBeVisible();
  await expect(page.getByTestId("protocol-advisory-banner")).toHaveCount(0);

  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.protocolCompatibility = { networkVersion: 16, supportedVersion: 15 };
    f.emit({
      kind: "platform",
      network: f.world.launch.network.dirName,
      walletId: f.world.wallets[0].summary.walletId,
      change: { domain: "startup" },
    });
  });

  const banner = page.getByTestId("protocol-advisory-banner");
  await expect(banner).toBeVisible();
  await page.getByTestId("protocol-advisory-link").click();
  await expect(page.getByTestId("protocol-advisory-error")).toHaveRole("alert");
  await expect(page.getByTestId("protocol-advisory-error")).toHaveText("Could not open the releases page.");
  expect(errors).toEqual([]);
});
