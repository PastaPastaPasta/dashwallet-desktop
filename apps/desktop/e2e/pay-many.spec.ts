import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// QT-052 on the fixture: Add recipient pays several addresses in one transaction; the confirm sheet
// lists each with what it receives; one recipient at most subtracts the fee; the signed transaction
// pays each exactly that (src/fixture/send.ts). Testnet only (`multiRecipientPay`): mainnet shows
// no Add recipient. DW_ROUTE_SHOTS=<dir> (scripts/e2e-docker.sh) also writes the shots there.

const SECOND = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const shots = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario = "funded", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-pay").click();
  return errors;
}

async function shot(page: Page, name: string, theme: Theme) {
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  if (!shots) return;
  mkdirSync(shots, { recursive: true });
  await page.screenshot({ path: `${shots}/${name}-${theme}.png`, fullPage: true });
}

const world = <T>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

for (const theme of THEMES)
  test(`pays two recipients in one transaction ${theme}: each row, the confirm list, the signed outputs`, async ({ page }) => {
    const errors = await open(page, "funded", theme);
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.3");
    await page.getByTestId("pay-label").fill("Alice");
    await page.getByTestId("pay-add-recipient").click();
    await expect(page.getByTestId("pay-more-2")).toBeVisible();
    await page.getByTestId("pay-more-2-address").fill(SECOND);
    await page.getByTestId("pay-more-2-amount").fill("0.2");
    await page.getByTestId("pay-more-2-label").fill("Bob");
    // One recipient at most subtracts the fee: Bob's turns Alice's off.
    await page.getByTestId("pay-subtract-fee").check();
    await page.getByTestId("pay-more-2-subtract-fee").check();
    await expect(page.getByTestId("pay-subtract-fee")).not.toBeChecked();
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await shot(page, "pay-many-form", theme);
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("pay-confirm")).toBeVisible();
    await expect(page.getByTestId("pay-confirm-recipient-0")).toContainText("Alice");
    await expect(page.getByTestId("pay-confirm-recipient-0")).toContainText("0.30000000");
    await expect(page.getByTestId("pay-confirm-recipient-1")).toContainText("Bob");
    const fee = Number((await page.getByTestId("pay-confirm-fee").innerText()).replace(/[^0-9.]/g, ""));
    await expect(page.getByTestId("pay-confirm-recipient-1")).toContainText((0.2 - fee).toFixed(8));
    await expect(page.getByTestId("pay-confirm-subtract-many")).toContainText("of the fee is subtracted from Bob's amount");
    await shot(page, "pay-many-confirm", theme);
    await page.getByTestId("pay-confirm-passphrase").fill("demo");
    await page.getByTestId("pay-confirm-send").click();
    await expect(page.getByTestId("pay-result-sent")).toBeVisible();
    // "Sent" means the signed outputs kept the sheet's promise (keepsPromise): each paid exactly
    // what its row showed. The draft held both recipients, Bob's subtracting the fee.
    const set = await world(page, (w) => Object.values(w.send.drafts).at(-1)!);
    expect(set.recipients.map((r) => [r.address, r.amount, r.subtractFeeFromAmount])).toEqual([
      [PAYEE, 30_000_000, false],
      [SECOND, 20_000_000, true],
    ]);
    expect(await world(page, (w) => w.grants.at(-1)?.grant.purpose)).toEqual({ kind: "spend", maxDuffs: 50_000_000 });
    expect(errors).toEqual([]);
  });

test("a removed row leaves the payment, a refused row says so, and mainnet has no Add recipient", async ({ page }) => {
  await open(page);
  await page.getByTestId("pay-address").fill(PAYEE);
  await page.getByTestId("pay-amount").fill("0.1");
  await page.getByTestId("pay-add-recipient").click();
  await page.getByTestId("pay-more-2-address").fill(PAYEE);
  await page.getByTestId("pay-more-2-amount").fill("0.1");
  // The same address twice: the engine refuses it, on the repeated row.
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  await expect(page.getByTestId("pay-problem")).toBeVisible();
  await expect(page.getByTestId("pay-more-2")).toContainText(await page.getByTestId("pay-problem").innerText());
  // Max waits for every row's amount, and pays around it.
  await page.getByTestId("pay-more-2-address").fill("");
  await page.getByTestId("pay-max").click();
  await expect.poll(async () => Number(await page.getByTestId("pay-amount").inputValue())).toBeGreaterThan(0);
  const max = Number(await page.getByTestId("pay-amount").inputValue());
  await page.getByTestId("pay-amount").fill("0.1");
  await page.getByTestId("pay-more-2-remove").click();
  await page.getByTestId("pay-max").click();
  await expect.poll(async () => Number(await page.getByTestId("pay-amount").inputValue())).toBeCloseTo(max + 0.1, 6);
  await page.getByTestId("pay-amount").fill("0.1");
  await page.getByTestId("pay-add-recipient").click();
  await page.getByTestId("pay-more-2-address").fill(PAYEE);
  await page.getByTestId("pay-more-2-amount").fill("0.1");
  await page.getByTestId("pay-more-2-remove").click();
  await expect(page.getByTestId("pay-more-2")).toHaveCount(0);
  await expect(page.getByTestId("pay-continue")).toBeEnabled();

  await page.goto(fixtureUrl("funded", "light", "&network=mainnet"));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-pay").click();
  await expect(page.getByTestId("pay-address")).toBeVisible();
  await expect(page.getByTestId("pay-add-recipient")).toHaveCount(0);
});
