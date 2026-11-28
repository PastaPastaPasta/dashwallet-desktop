import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";

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

async function openCoinJoin(page: Page) {
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
}

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

test("MIX-SHIELD Tools ▸ Repair lists the possibly sent move, sends its coins back at the fee it showed, and the next move runs", async ({ page }) => {
  const errors = await openSheet(page, "coinjoin-move", "light", "coinjoin-move-shield");
  await page.evaluate(() => void (window.__dwFixture!.world.coinjoin.drainUnknown = "next"));
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-step", "refused");
  await page.getByTestId("move-mixed-close").click();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "repair" }));
  const row = page.getByTestId("repair-stuck-shield/0/funding");
  await expect(row).toContainText("A move of mixed coins into Shielded may have been sent.");
  await expect(row).toContainText(/Repair sends the move's 3 mixed coins \(0\.300003 t?DASH\) back to your wallet's main balance, at a network fee of 0\.00000488 t?DASH\./);
  await page.getByTestId("repair-resolve-shield/0/funding").click();
  const sheet = page.getByTestId("repair-confirm");
  await expect(sheet).toContainText(/You pay a network fee of 0\.00000488 t?DASH\./);
  await expect(page.getByTestId("repair-confirm-ok")).toHaveText(/^Send back and pay 0\.00000488 t?DASH$/);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("repair-confirm-cancel").click();
  await expect(row).toBeVisible();
  await page.getByTestId("repair-resolve-shield/0/funding").click();
  await page.getByTestId("repair-confirm-ok").click();
  await authorize(page);
  await expect(page.getByTestId("repair-outcome")).toContainText("You can move Dash into Shielded again once it is confirmed.");
  await expect(row).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "coinJoin" }));
  await page.getByTestId("coinjoin-move-shield").click();
  await expect(body(page)).toHaveAttribute("data-step", "confirm");
  await page.getByTestId("move-mixed-confirm").click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-outcome", "shielded");
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

// MG: Move to Shielded waits as Add does, names the cause and offers the unlock that lets it run.
for (const [scenario, why, unlock] of [
  ["coinjoin-move-shielded-locked", "Shielded starts syncing when you unlock the wallet. Unlock it, then try again once the sync finishes.", "Unlock"],
  ["coinjoin-move-shielded-mixing-only", "The wallet is unlocked for mixing only, so Shielded can't sync. Unlock it fully, then try again once the sync finishes.", "Unlock fully"],
  ["coinjoin-move-shielded-watch-only", "Shielded isn't available in a watch-only wallet.", null],
] as const) {
  test(`MIX-SHIELD the card's Move to Shielded waits and names why: ${scenario}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl(scenario, "light"));
    await openCoinJoin(page);
    const shield = page.getByTestId("coinjoin-move-shield");
    await expect(shield).toBeDisabled();
    await expect(page.locator(".dash-button-why", { hasText: why })).toBeVisible();
    if (unlock === null) await expect(page.getByTestId("coinjoin-move-unlock")).toHaveCount(0);
    else await expect(page.getByTestId("coinjoin-move-unlock")).toHaveText(unlock);
    expect(errors).toEqual([]);
  });
}

test("MIX-SHIELD Unlock fully on the card lets Shielded sync, and the card waits on the sync alone", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("coinjoin-move-shielded-mixing-only", "light"));
  await openCoinJoin(page);
  await page.getByTestId("coinjoin-move-unlock").click();
  await page.getByTestId("unlock-sheet-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("unlock-sheet-submit").click();
  await expect(page.getByTestId("coinjoin-move-unlock")).toHaveCount(0);
  await expect(page.locator(".dash-button-why", { hasText: "Shielded is still syncing. Try again when it finishes." })).toBeVisible();
  expect(errors).toEqual([]);
});

for (const theme of THEMES) {
  test(`Return to Core from CoinJoin page ${theme}: held coins with zero free CoinJoin balance, confirm amount and destination, authorize, done; 0 serious axe issues`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("coinjoin-held", theme));
    await openCoinJoin(page);
    await expect(page.getByTestId("coinjoin-move")).toBeVisible();
    await expect(page.getByTestId("coinjoin-move-open")).toHaveCount(0);
    await expect(page.getByTestId("coinjoin-held-note")).toContainText("0.200402");
    await page.getByTestId("coinjoin-return-open").click();
    await expect(page.getByTestId("return-to-core-sheet")).toBeVisible();
    await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-step", "confirm");
    await expect(page.getByTestId("return-to-core-amount")).toContainText("0.200402");
    await expect(page.getByTestId("return-to-core-destination")).toHaveText("Core wallet");
    await expect(page.getByTestId("return-to-core-fee")).toBeVisible();
    await expect(page.getByTestId("return-to-core-receive")).toBeVisible();
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("return-to-core-confirm").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    await authorize(page);
    await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-outcome", "done");
    await expect(page.getByTestId("return-to-core-outcome-title")).toHaveText("Coins returning to Core");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("return-to-core-close").click();
    await expect(page.getByTestId("return-to-core-sheet")).toHaveCount(0);
    await expect(page.getByTestId("coinjoin-move")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`Return to Core from Settings ▸ CoinJoin ${theme}: held coins card, confirm, authorize, done; 0 serious axe issues`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("coinjoin-held", theme));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await page.getByTestId("settings-tabs").getByRole("tab", { name: "CoinJoin" }).click();
    await expect(page.getByTestId("settings-coinjoin-return")).toBeVisible();
    await expect(page.getByTestId("settings-coinjoin-held-note")).toContainText("0.200402");
    await page.getByTestId("settings-coinjoin-return-open").click();
    await expect(page.getByTestId("return-to-core-sheet")).toBeVisible();
    await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-step", "confirm");
    await expect(page.getByTestId("return-to-core-amount")).toContainText("0.200402");
    await expect(page.getByTestId("return-to-core-destination")).toHaveText("Core wallet");
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("return-to-core-confirm").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    await authorize(page);
    await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-outcome", "done");
    await page.getByTestId("return-to-core-close").click();
    await expect(page.getByTestId("settings-coinjoin-return")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("Return to Core while mixing offers Stop Mixing first", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("coinjoin-held-mixing", "light"));
  await openCoinJoin(page);
  await page.getByTestId("coinjoin-return-open").click();
  await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-step", "mixing");
  await page.getByTestId("return-to-core-stop").click();
  await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-step", "confirm");
  await expect(page.getByTestId("return-to-core-amount")).toContainText("0.200402");
  expect(errors).toEqual([]);
});

test("Return to Core with unknown outcome offers Try again (resend)", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("coinjoin-held-unknown", "light"));
  await openCoinJoin(page);
  await page.getByTestId("coinjoin-return-open").click();
  await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-step", "confirm");
  await page.getByTestId("return-to-core-confirm").click();
  await expect(page.getByTestId("grant-sheet")).toBeVisible();
  await authorize(page);
  await expect(page.getByTestId("return-to-core-body")).toHaveAttribute("data-outcome", "unknown");
  await page.getByTestId("return-to-core-resend").click();
  await expect(page.getByTestId("return-to-core-resent")).toHaveAttribute("data-state", "unknown");
  expect(errors).toEqual([]);
});
