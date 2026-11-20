import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_WALLET_ID, SECOND_WALLET_ID } from "../src/fixture/scenarios";
import { WalletsText } from "../src/settings/wallets";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// #113: Settings ▸ Wallets keeps one wallet on the load-on-startup list, as it keeps one open.

const A = `settings-wallet-${FIXTURE_WALLET_ID.slice(0, 8)}`;
const B = `settings-wallet-${SECOND_WALLET_ID.slice(0, 8)}`;

async function walletsTab(page: Page, theme: Theme) {
  await page.goto(fixtureUrl("two-wallets", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-wallets").click();
  await page.getByTestId(`${A}-startup`).waitFor();
}

for (const theme of THEMES)
  test(`the last wallet opening at start cannot be unticked (${theme})`, async ({ page }, info) => {
    const errors = trackPageErrors(page);
    await walletsTab(page, theme);
    const a = page.getByTestId(`${A}-startup`);
    const b = page.getByTestId(`${B}-startup`);
    await expect(a).toBeEnabled();
    await expect(page.getByTestId(`${A}-startup-why`)).toHaveCount(0);

    await b.click();
    await expect(b).not.toBeChecked();
    await expect(a).toBeChecked();
    await expect(a).toBeDisabled();
    await expect(a).toHaveAccessibleDescription(WalletsText.oneOpensAtStart);
    await expect(page.getByTestId(`${A}-startup-why`)).toHaveText(WalletsText.oneOpensAtStart);
    await expect(b).toBeEnabled();
    await expect(page.getByTestId(`${B}-startup-why`)).toHaveCount(0);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("settings-wallets").screenshot({ path: artifactPath(info, `last-startup-wallet-${theme}.png`) });

    await b.click();
    await expect(b).toBeChecked();
    await expect(a).toBeEnabled();
    await expect(page.getByTestId(`${A}-startup-why`)).toHaveCount(0);
    expect(errors).toEqual([]);
  });

test("a refusal from the backend is shown and changes nothing", async ({ page }) => {
  const errors = trackPageErrors(page);
  await walletsTab(page, "light");
  // Another window (or dwcli) took B off the list after this screen read it.
  await page.evaluate((id) => window.__dwFixture!.world.settings.notAtStartup.push(id), SECOND_WALLET_ID);
  const a = page.getByTestId(`${A}-startup`);
  await expect(a).toBeEnabled();
  await a.click();
  await expect(page.getByTestId("settings-wallets-error")).toHaveText(WalletsText.oneOpensAtStart);
  await expect(a).toBeChecked();
  await expect(a).toBeDisabled();
  expect(await page.evaluate(() => window.__dwFixture!.world.settings.notAtStartup)).toEqual([SECOND_WALLET_ID]);
  expect(errors).toEqual([]);
});
