import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// "Move mixed coins" (IOS-057) on the fixture backend: the card on the CoinJoin
// page; to the wallet, the plan with its fully and not fully mixed amounts, Stop Mixing when mixing
// runs, the grant prompt, and the complete, partial and unknown outcomes; into Shielded, the lock's
// quote and outcome, and the consolidation round above 50 coins; 0 serious axe issues in light and
// dark.

async function openSheet(page: Page, scenario: string, theme: Theme = "light", button = "coinjoin-move-open") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await page.getByTestId(button).click();
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

for (const theme of THEMES) {
  test(`MIX-SHIELD move mixed coins to Shielded ${theme}: quote, grant, outcome; 0 serious axe issues`, async ({ page }) => {
    const errors = await openSheet(page, "coinjoin-move", theme, "coinjoin-move-shield");
    await expect(body(page)).toHaveAttribute("data-step", "confirm");
    await expect(body(page)).toHaveAttribute("data-destination", "shielded");
    await expect(page.getByTestId("move-mixed-fully")).toContainText("0.200002");
    await expect(page.getByTestId("move-mixed-partly")).toContainText("0.100001");
    await expect(page.getByTestId("move-mixed-pool-fee")).toBeVisible();
    await expect(page.getByTestId("move-mixed-plan")).toContainText("Network fee, at most");
    await expect(page.getByTestId("move-mixed-plan")).toContainText("Added to Shielded, at least");
    await expect(page.getByTestId("move-mixed-combined")).toBeVisible();
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("move-mixed-confirm").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    await authorize(page);
    await expect(body(page)).toHaveAttribute("data-outcome", "shielded");
    await expect(page.getByTestId("move-mixed-outcome-title")).toHaveText("Added to Shielded");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("MIX-SHIELD coins that changed as the lock was built show a fresh quote, and the next Move goes", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move", "light", "coinjoin-move-shield");
  await expect(body(page)).toHaveAttribute("data-step", "confirm");
  await page.evaluate(() => void (window.__dwFixture!.world.coinjoin.drainChanged = true));
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(page.getByTestId("move-mixed-replanned")).toHaveText("Your CoinJoin balance changed since you reviewed this move. Review it again.");
  await expect(body(page)).toHaveAttribute("data-step", "confirm");
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-outcome", "shielded");
  expect(errors).toEqual([]);
});

test("MIX-SHIELD a lock that signed with no verdict reads possibly sent and points to Tools ▸ Repair", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move", "light", "coinjoin-move-shield");
  await expect(body(page)).toHaveAttribute("data-step", "confirm");
  await page.evaluate(() => void (window.__dwFixture!.world.coinjoin.drainUnknown = "next"));
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-step", "refused");
  await expect(page.getByTestId("move-mixed-refusal")).toHaveText("The payment may have been sent. Open Tools ▸ Repair to settle it; you can move Dash into Shielded again once it has.");
  await expect(page.getByTestId("move-mixed-again")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("MIX-SHIELD above 50 coins, combine first, then wait for them to confirm", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move-many", "light", "coinjoin-move-shield");
  await expect(body(page)).toHaveAttribute("data-step", "combine");
  await expect(page.getByTestId("move-mixed-combine-text")).toContainText("holds 60 coins");
  await expect(page.getByTestId("move-mixed-coins-after")).toHaveText("1");
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("move-mixed-combine").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-outcome", "done");
  await expect(page.getByTestId("move-mixed-outcome-title")).toHaveText("Coins combined");
  await page.getByTestId("move-mixed-shield-next").click();
  await expect(body(page)).toHaveAttribute("data-step", "refused");
  await expect(page.getByTestId("move-mixed-refusal")).toContainText("still confirming");
  expect(errors).toEqual([]);
});

test("MIX-SHIELD no Move to Shielded where the network has no Shielded", async ({ page }) => {
  await page.goto(fixtureUrl("coinjoin-move-no-shielded", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await expect(page.getByTestId("coinjoin-move-open")).toBeVisible();
  await expect(page.getByTestId("coinjoin-move-shield")).toHaveCount(0);
});

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
