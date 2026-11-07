import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// "Move mixed coins" to the wallet (IOS-057, 0.2 MIX-SHIELD unit A) on the fixture backend: the
// card on the CoinJoin page, the plan with its fully and not fully mixed amounts, Stop Mixing when
// mixing runs, the grant prompt, and the complete, partial and unknown outcomes; 0 serious axe
// issues in light and dark.

async function openSheet(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await page.getByTestId("coinjoin-move-open").click();
  await expect(page.getByTestId("move-mixed-sheet")).toBeVisible();
  return errors;
}

const body = (page: Page) => page.getByTestId("move-mixed-body");

for (const theme of THEMES) {
  test(`IOS-057 move mixed coins ${theme}: plan, grant, outcome; 0 serious axe issues`, async ({ page }) => {
    const errors = await openSheet(page, "coinjoin-move", theme);
    await expect(body(page)).toHaveAttribute("data-step", "confirm");
    // Two coins of 0.100001 with the network's rounds, one short of them.
    await expect(page.getByTestId("move-mixed-fully")).toContainText("0.200002");
    await expect(page.getByTestId("move-mixed-partly")).toContainText("0.100001");
    await expect(page.getByTestId("move-mixed-combined")).toHaveText("These coins are combined on-chain, so the fully mixed amount loses some of its mixing privacy.");
    await expect(page.getByTestId("move-mixed-transactions")).toHaveText("This takes one transaction.");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("move-mixed-confirm").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    await authorize(page);
    await expect(body(page)).toHaveAttribute("data-outcome", "done");
    await expect(page.getByTestId("move-mixed-outcome-title")).toHaveText("Coins moved");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("move-mixed-close").click();
    await expect(page.getByTestId("move-mixed-sheet")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`IOS-057 move mixed coins while mixing ${theme}: Stop Mixing first; 0 serious axe issues`, async ({ page }) => {
    const errors = await openSheet(page, "coinjoin-move-mixing", theme);
    await expect(body(page)).toHaveAttribute("data-step", "mixing");
    await expect(page.getByTestId("move-mixed-mixing")).toContainText("Stop mixing first");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("move-mixed-stop").click();
    await expect(body(page)).toHaveAttribute("data-step", "confirm");
    await expect(page.locator("[data-testid=coinjoin-status]")).not.toHaveAttribute("data-state", "mixing");
    expect(errors).toEqual([]);
  });
}

test("IOS-057 a partial move says what moved and offers Try again", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move-partial");
  await expect(page.getByTestId("move-mixed-transactions")).toHaveText("This takes 2 transactions.");
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-outcome", "partial");
  await expect(page.getByTestId("move-mixed-outcome-text")).toContainText("stayed in your CoinJoin balance");
  await page.getByTestId("move-mixed-again").click();
  await expect(body(page)).toHaveAttribute("data-step", "confirm");
  await expect(page.getByTestId("move-mixed-transactions")).toHaveText("This takes one transaction.");
  expect(errors).toEqual([]);
});

test("IOS-057 a chunk of unknown outcome keeps its coins held", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move-unknown");
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-outcome", "unknown");
  await expect(page.getByTestId("move-mixed-outcome-text")).toContainText("may have reached the network");
  expect(errors).toEqual([]);
});

test("IOS-057 no move card for a wallet that holds no mixing coins", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await page.getByTestId("coinjoin-levels").locator("fieldset").waitFor();
  await expect(page.getByTestId("coinjoin-move")).toHaveCount(0);
});
