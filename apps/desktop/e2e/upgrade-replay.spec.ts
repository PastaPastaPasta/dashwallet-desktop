import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// #172: the first start after an update from 0.1, 0.2 or 0.3.0 replays a wallet once from its birth,
// and sends wait for it. Home and Pay say why, for that wallet only; once it is done, the notice goes.
// DW_ROUTE_SHOTS=<dir> (passed through by scripts/e2e-docker.sh) also writes each screenshot there.

const shots = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario: string, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("hero").waitFor();
  return errors;
}

async function shoot(page: Page, name: string) {
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await fontsReady(page);
  await page.screenshot({ path: `${shots}/${name}.png` });
}

for (const theme of THEMES) {
  test(`Home and Pay say why sends wait during an upgrade replay (${theme})`, async ({ page }) => {
    const errors = await open(page, "upgrade-replay", theme);
    const notice = page.getByTestId("upgrade-replay-notice");
    await expect(notice).toBeVisible();
    await expect(page.getByTestId("upgrade-replay-text")).toContainText("rescans it once from its birth");
    await shoot(page, `home-upgrade-replay-${theme}`);
    await page.getByTestId("sidebar-pay").click();
    await expect(notice).toBeVisible();
    await shoot(page, `pay-upgrade-replay-${theme}`);
    expect(errors).toEqual([]);
  });
}

test("the notice goes once this wallet is no longer replayed for the upgrade", async ({ page }) => {
  await open(page, "upgrade-replay", "light");
  await page.getByTestId("sidebar-pay").click();
  const notice = page.getByTestId("upgrade-replay-notice");
  await expect(notice).toBeVisible();
  // Another wallet still replays for the upgrade; this one rescans for another reason.
  await page.evaluate(() => {
    const r = window.__dwFixture!.world.tools.rescan!;
    r.upgradeWallets = ["00".repeat(32)];
  });
  await expect(notice).toHaveCount(0, { timeout: 10_000 });
});
