import { expect, test, type Page } from "@playwright/test";
import { artifactPath, fixtureUrl, fontsReady, openToolsTab, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// A coin held by a block-spend check (`checking`, scenario coins-checking): its coin-control row says
// it is being checked and cannot be chosen, in Pay ▸ Advanced ▸ Selected coins and in Tools ▸ Coin
// Selection; the other coins keep their reasons and stay choosable.

const CHECKING = "Checking whether it was spent elsewhere";

async function open(page: Page, theme: Theme) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("coins-checking", theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function rowIds(page: Page) {
  return page.evaluate(() => {
    const id = (u: { outpoint: { txid: string; vout: number } }) => `coin-${u.outpoint.txid.slice(0, 8)}-${u.outpoint.vout}`;
    const coins = window.__dwFixture!.world.wallets[0].coins;
    return { checking: coins.filter((u) => u.checking).map(id), others: coins.filter((u) => !u.checking).map(id), unconfirmed: coins.filter((u) => u.confirmations === 0).map(id) };
  });
}

async function expectRows(page: Page) {
  const table = page.getByTestId("coin-table");
  const { checking, others, unconfirmed } = await rowIds(page);
  expect(checking).toHaveLength(1);
  const row = table.getByTestId(checking[0]);
  await expect(row).toContainText(CHECKING);
  await expect(row).not.toContainText("Not spendable");
  await expect(row).not.toContainText("Locked");
  await expect(row.locator("input[type=checkbox]").first()).toBeDisabled();
  await expect(table.getByText(CHECKING)).toHaveCount(1);
  for (const id of others) {
    const other = table.getByTestId(id);
    await expect(other).not.toContainText(CHECKING);
    if (unconfirmed.includes(id)) {
      await expect(other).toContainText("Not spendable");
      await expect(other.locator("input[type=checkbox]").first()).toBeDisabled();
    } else await expect(other.locator("input[type=checkbox]").first()).toBeEnabled();
  }
  return row;
}

for (const theme of THEMES) {
  test(`a checking coin in Pay ▸ Advanced ▸ Selected coins, ${theme}`, async ({ page }, info) => {
    const errors = await open(page, theme);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    await page.getByTestId("pay-fee-line").waitFor();
    await page.getByTestId("pay-advanced").locator("summary").click();
    await page.getByTestId("coin-choice-selected").check();
    await expectRows(page);
    await fontsReady(page);
    await page.getByTestId("pay-advanced").screenshot({ path: artifactPath(info, `${theme}-coin-control-checking-pay.png`) });
    expect(errors).toEqual([]);
  });

  test(`a checking coin in Tools ▸ Coin Selection, ${theme}`, async ({ page }, info) => {
    const errors = await open(page, theme);
    await openToolsTab(page, "coinSelection");
    const row = await expectRows(page);
    const fits = await row.locator("td.coin-status").evaluate((cell) => {
      const panel = cell.closest("[data-testid=tools-coinSelection]")!.getBoundingClientRect();
      return cell.getBoundingClientRect().right <= panel.right + 0.5 && cell.scrollWidth <= cell.clientWidth;
    });
    expect(fits).toBe(true);
    await page.getByTestId("coins-select-all").click();
    await expect(row.locator("input[type=checkbox]").first()).not.toBeChecked();
    await expect(page.getByTestId("coin-summary")).toContainText("Quantity3");
    await fontsReady(page);
    await page.getByTestId("tools-coinSelection").screenshot({ path: artifactPath(info, `${theme}-coin-control-checking-tools.png`) });
    expect(errors).toEqual([]);
  });
}
