import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, openTransfer, trackPageErrors } from "./helpers";
import { amountsInClear, callsOf, FEE_ON_NEXT_STEP, hold, lastGrant, moveProgressAt, release, TRANSFER_STATES } from "./transfer";

// Transfer (POLISH 25): one sheet over the four routes' own flows. Each state of transfer.ts with
// no serious axe violation and no page error; each route's grant as its own sheet asked for it.

async function open(page: Page, scenario: string, extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, "light", extra));
  await page.getByTestId("hero").waitFor();
  return errors;
}

test.describe("Transfer states", () => {
  for (const s of TRANSFER_STATES) {
    test(s.id, async ({ page }) => {
      const errors = await open(page, s.scenario, s.extra);
      await s.go(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
});

test.describe("Live transfer steps", () => {
  for (const route of ["coreToShielded", "shieldedToCore", "shieldedToPlatform"] as const) {
    test(`${route} keeps its checklist through confirmation`, async ({ page }) => {
      await open(page, "funded");
      await moveProgressAt(page, route, "platformSubmission");
      const list = page.getByRole("list", { name: "Transfer steps" });
      await expect(list.locator('[data-state="done"]')).toHaveCount(route === "coreToShielded" ? 3 : 2);
      await expect(list.locator('[data-stage="refreshShielded"]')).toHaveAttribute("data-state", "upcoming");
      await release(page);
      await expect(page.getByTestId("shielded-outcome-title")).toBeVisible();
      await expect(list.locator('[data-state="done"]')).toHaveCount(route === "coreToShielded" ? 5 : 4);
    });
  }

  test("a network refusal attaches its error to the step that failed", async ({ page }) => {
    const state = TRANSFER_STATES.find((state) => state.id === "shieldedToCore-failed")!;
    await open(page, state.scenario);
    await state.go(page);
    const failed = page.getByTestId("shielded-progress").locator('[data-state="failed"]');
    await expect(failed).toHaveAttribute("data-stage", "platformSubmission");
    await expect(failed.getByRole("alert")).toHaveText("Dash Platform did not answer. Try again.");
  });

  test("a Core lock without a proof keeps that step waiting", async ({ page }) => {
    const state = TRANSFER_STATES.find((state) => state.id === "coreToShielded-pending")!;
    await open(page, state.scenario);
    await state.go(page);
    const list = page.getByTestId("shielded-progress");
    await expect(list.locator('[data-stage="coreAssetLock"]')).toHaveAttribute("data-state", "waiting");
    await expect(list.locator('[data-stage="shieldedProof"]')).toHaveAttribute("data-state", "upcoming");
  });

  test("the successful command reply cannot reveal a final amount before the settlement event", async ({ page }) => {
    await open(page, "funded");
    await page.evaluate(() => {
      (window as unknown as { __settleMove: () => void }).__settleMove = window.__dwFixture!.holdMoveSettlement();
    });
    await moveProgressAt(page, "coreToShielded", "refreshShielded");
    await expect(page.getByTestId("shielded-outcome-text")).toHaveCount(0);
    // Let the command answer but keep the settled event held.
    await release(page);
    await page.evaluate(() => window.__dwFixture!.emit({ kind: "eventsOverflowed" }));
    await expect(page.getByTestId("shielded-body")).toHaveAttribute("data-step", "checkActivity");
    await expect(page.getByTestId("shielded-outcome-text")).toHaveCount(0);
    await page.evaluate(() => (window as unknown as { __settleMove: () => void }).__settleMove());
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await expect(page.getByTestId("shielded-outcome-text")).toContainText("Your Shielded balance is now");
  });
});

/** Through the route's confirmation to its passphrase sheet; the grant asked for, once answered. */
async function grantOf(page: Page, go: string) {
  await page.getByTestId("transfer-continue").click();
  await page.getByTestId(go).click();
  await expect(page.getByTestId("grant-sheet")).toBeVisible();
  await expect(page.getByTestId("grant-sheet")).toContainText("Enter your passphrase to authorize this payment.");
  await authorize(page);
  return lastGrant(page);
}

test.describe("Transfer grants", () => {
  test("Dash wallet → Shielded: a platformOp grant of the lock and its fee bound", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").fill("0.001");
    expect(await grantOf(page, "shielded-go")).toEqual({ kind: "platformOp", maxDuffs: 101_000, maxCredits: "0" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
  });

  test("Dash wallet → Platform: a platformOp grant of the lock and its fee bound", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToPlatform");
    await page.getByTestId("transfer-amount").fill("0.5");
    expect(await grantOf(page, "platform-fund-go")).toEqual({ kind: "platformOp", maxDuffs: 50_001_000, maxCredits: "0" });
    await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
  });

  // Max from the Dash wallet: one quote of the whole account, and a grant of everything the lock spends
  // (the fee out of it), so nothing comes back as change.
  for (const [route, go, quote, done] of [
    ["coreToShielded", "shielded-go", "shield_quote", "shielded-outcome-title"],
    ["coreToPlatform", "platform-fund-go", "platform_fund_quote", "platform-fund-outcome-title"],
  ] as const)
    test(`Dash wallet → ${route === "coreToShielded" ? "Shielded" : "Platform"} Max: a platformOp grant of every coin it locks`, async ({ page }) => {
      await open(page, "funded");
      await openTransfer(page, route);
      await page.getByTestId("transfer-max").click();
      await expect(page.getByTestId("transfer-continue")).toBeEnabled();
      expect(await callsOf(page, quote)).toBe(1);
      expect(await grantOf(page, go)).toEqual({ kind: "platformOp", maxDuffs: 3_019_698_287, maxCredits: "0" });
      await expect(page.getByTestId(done)).toHaveText(route === "coreToShielded" ? "Added to Shielded" : "Added to Platform");
    });

  for (const [route, go, amount, maxDuffs] of [
    ["coreToShielded", "shielded-go", "0.001", 101_000],
    ["coreToPlatform", "platform-fund-go", "0.5", 50_001_000],
  ] as const)
    test(`Dash wallet → ${route === "coreToShielded" ? "Shielded" : "Platform"}: an amount edited after Max is an amount, its fee on top`, async ({ page }) => {
      await open(page, "funded");
      await openTransfer(page, route);
      await page.getByTestId("transfer-max").click();
      await expect(page.getByTestId("transfer-continue")).toBeEnabled();
      await page.getByTestId("transfer-amount").fill(amount);
      expect(await grantOf(page, go)).toEqual({ kind: "platformOp", maxDuffs, maxCredits: "0" });
    });

  test("Shielded → Dash wallet: a platformOp grant of the credits and the fee", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "shieldedToCore");
    await page.getByTestId("transfer-amount").fill("0.5");
    expect(await grantOf(page, "shielded-go")).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "50020000000" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
  });

  test("Shielded → Platform: a platformOp grant of the credits and the fee, paid to the receive address", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "shieldedToPlatform");
    await page.getByTestId("transfer-amount").fill("0.5");
    expect(await grantOf(page, "shielded-go")).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "50026082800" });
    // The fee is the spend's 20000000 plus the flat 6082800 of the Platform address it credits.
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
    expect(await callsOf(page, "unshield_to_platform")).toBe(1);
    expect(await callsOf(page, "unshield_to_core")).toBe(0);
    // Platform's balance follows: Home's breakdown shows the 0.5 the move added.
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.90");
  });

  test("Platform → Dash wallet: a platformOp grant of the whole balance", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "platformToCore");
    expect(await grantOf(page, "platform-move-go")).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "40000000000" });
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
  });

  test("a card's Finish opens Transfer at its route, with the one-duff grant", async ({ page }) => {
    await open(page, "transfer-shield-pending");
    await page.getByTestId("shielded-finish").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "coreToShielded");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-view", "steps");
    await authorize(page);
    expect(await lastGrant(page)).toEqual({ kind: "platformOp", maxDuffs: 1, maxCredits: "0" });
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
  });
});

test.describe("Transfer quotes", () => {
  test("typing reads one quote, for the amount it settles on", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").pressSequentially("0.0015", { delay: 40 });
    await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about /);
    expect(await callsOf(page, "shield_quote")).toBe(1);
  });

  test("Dash wallet → Platform: 0 platform_fund_quote calls while typing, 1 per Max click, 1 on Continue", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "coreToPlatform");
    const amount = page.getByTestId("transfer-amount");
    await amount.pressSequentially("0.55", { delay: 40 });
    await expect(page.getByTestId("transfer-fee")).toHaveText(FEE_ON_NEXT_STEP);
    // Well past the other routes' debounce: still nothing read.
    await page.waitForTimeout(1_500);
    expect(await callsOf(page, "platform_fund_quote")).toBe(0);
    // Max: one quote of the whole account.
    await page.getByTestId("transfer-max").click();
    await expect(amount).not.toHaveValue("0.55");
    await expect(amount).toHaveValue(/^\d+\.\d+$/);
    await page.waitForTimeout(1_500);
    expect(await callsOf(page, "platform_fund_quote")).toBe(1);
    await page.getByTestId("transfer-max").click();
    await expect(page.getByTestId("transfer-continue")).toBeEnabled();
    await page.waitForTimeout(1_500);
    expect(await callsOf(page, "platform_fund_quote")).toBe(2);
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("platform-fund-confirm")).toContainText("You receive about");
    expect(await callsOf(page, "platform_fund_quote")).toBe(3);
  });

  test("Dash wallet → Platform: a refused Max leaves the amount typed", async ({ page }) => {
    await open(page, "transfer-many-coins");
    await openTransfer(page, "coreToPlatform");
    const amount = page.getByTestId("transfer-amount");
    await amount.fill("0.3");
    await page.getByTestId("transfer-max").click();
    await expect(page.getByTestId("transfer-fee")).toHaveText(/^Max needs at most 50 coins; this wallet has 60\. Enter an amount instead\.$/);
    await expect(amount).toHaveValue("0.3");
    expect(await callsOf(page, "platform_fund_quote")).toBe(1);
  });

  test("discreet mode: no amount shows in clear on any route, Max's fill and the whole balance included", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("toolbar-discreet").click();
    for (const route of ["coreToShielded", "coreToPlatform", "shieldedToCore", "platformToCore"] as const) {
      await openTransfer(page, route);
      if (route === "platformToCore") await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about /);
      else {
        await page.getByTestId("transfer-max").click();
        await expect(page.getByTestId("transfer-continue")).toBeEnabled();
        await expect(page.getByTestId("transfer-amount")).toHaveValue(/^[#.,]+$/);
      }
      expect(await amountsInClear(page), route).toEqual([]);
      await page.getByTestId("transfer-cancel").click();
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    }
  });

  test("the shortcut before the balances are read waits, then opens on the default route", async ({ page }) => {
    await open(page, "funded", "&hold=shielded_status&hold=platform_addresses");
    await page.getByTestId("shortcut-transfer").click();
    await expect(page.getByTestId("transfer-loading")).toHaveText("Reading your balances…");
    await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
    await expect(page.getByTestId("transfer-amount")).toHaveCount(0);
    await page.evaluate(() => {
      window.__dwFixture!.release("shielded_status");
      window.__dwFixture!.release("platform_addresses");
    });
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "coreToShielded");
    await expect(page.getByTestId("transfer-loading")).toHaveCount(0);
    await expect(page.getByTestId("transfer-amount")).toBeEnabled();
    await expect(page.getByTestId("transfer-max")).toBeEnabled();
  });

  test("keyboard: every pick returns focus to the amount when the route can run, else to the end picked", async ({ page }) => {
    const focused = () => page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? document.activeElement?.tagName ?? null);
    const pick = async (end: "from" | "to", pool: string) => {
      const button = page.getByTestId(`transfer-${end}`);
      await button.focus();
      await page.keyboard.press("Enter");
      const list = page.getByTestId("transfer-picker");
      await expect(button).toHaveAttribute("aria-controls", (await list.getAttribute("id"))!);
      await page.getByTestId(`transfer-picker-${pool}`).focus();
      await page.keyboard.press("Enter");
      await expect(list).toHaveCount(0);
    };
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    // The same pool again: nothing changes, focus still lands.
    await pick("from", "core");
    await expect.poll(focused).toBe("transfer-amount");
    await pick("to", "platform");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "coreToPlatform");
    await expect.poll(focused).toBe("transfer-amount");
    // Shielded → Platform runs: focus on its amount.
    await pick("from", "shielded");
    await pick("to", "platform");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "shieldedToPlatform");
    await expect.poll(focused).toBe("transfer-amount");
    // Platform → Shielded has no command: no amount, focus back on To.
    await pick("from", "platform");
    await pick("to", "shielded");
    await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    await expect.poll(focused).toBe("transfer-to");
    await page.keyboard.press("Escape");
    // A route that can't run yet (Shielded syncing): its amount is disabled, focus back on To.
    await open(page, "shielded-syncing");
    await page.getByTestId("shortcut-transfer").click();
    await pick("to", "shielded");
    await expect(page.getByTestId("transfer-amount")).toBeDisabled();
    await expect.poll(focused).toBe("transfer-to");
  });

  // MG: Add and Transfer name the cause when the vault's lock or a watch-only wallet holds the account.
  for (const [scenario, line] of [
    ["shielded-locked", "Shielded starts syncing when you unlock the wallet. Unlock it, then try again once the sync finishes."],
    ["shielded-mixing-only", "The wallet is unlocked for mixing only, so Shielded can't sync. Unlock it fully, then try again once the sync finishes."],
    ["shielded-watch-only", "Shielded isn't available in a watch-only wallet."],
  ])
    test(`Add and Transfer out of Shielded name the cause in ${scenario}`, async ({ page }) => {
      await open(page, scenario);
      for (const route of ["coreToShielded", "shieldedToCore"] as const) {
        await openTransfer(page, route);
        await expect(page.getByTestId("transfer-unavailable")).toHaveText(line);
        await page.keyboard.press("Escape");
        await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      }
    });

  test("Home offers Transfer from its shortcut without duplicate pool cards", async ({ page }) => {
    await open(page, "funded");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    for (const card of ["shielded-card", "platform-card"]) await expect(page.getByTestId(card)).toHaveCount(0);
    await openTransfer(page, "shieldedToCore");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "shieldedToCore");
  });

  test("an amount typed on a route that then can't run is not quoted, and is quoted again once it can", async ({ page }) => {
    const ready = (on: boolean) =>
      page.evaluate(
        `(() => { const f = window.__dwFixture; f.world.pending.shielded.ready = ${on}; f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "startup" } }); f.emit({ kind: "eventsOverflowed" }); })()`,
      );
    await open(page, "funded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").fill("0.001");
    await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about /);
    expect(await callsOf(page, "shield_quote")).toBe(1);
    await ready(false);
    await expect(page.getByTestId("transfer-unavailable")).toHaveText("Shielded is still syncing. Try again when it finishes.");
    await expect(page.getByTestId("transfer-fee")).toHaveText("");
    await ready(true);
    await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
    await expect(page.getByTestId("transfer-fee")).toHaveText(/^Fee about /);
    expect(await callsOf(page, "shield_quote")).toBe(2);
  });

  test("a route's own run has no Running line; another route's has one, and its Show opens it", async ({ page }) => {
    await open(page, "funded");
    await hold(page, "platform_move_to_core");
    await openTransfer(page, "platformToCore");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-view", "steps");
    await expect(page.getByTestId("transfer-running")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    // Dash wallet → Shielded runs too: on its own steps, only Platform's line shows.
    await hold(page, "shield_from_core");
    await openTransfer(page, "coreToShielded");
    await expect(page.getByTestId("transfer-running")).toHaveAttribute("data-route", "platformToCore");
    await page.getByTestId("transfer-amount").fill("0.001");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-progress")).toBeVisible();
    await expect(page.getByTestId("transfer-running")).toHaveCount(1);
    await expect(page.getByTestId("transfer-running")).toHaveAttribute("data-route", "platformToCore");
    await page.getByTestId("transfer-running-show").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "platformToCore");
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-view", "steps");
    await expect(page.getByTestId("transfer-running")).toHaveAttribute("data-route", "coreToShielded");
    await expect(page.getByTestId("transfer-running")).toHaveCount(1);
  });

  test("Max never skips the confirmation", async ({ page }) => {
    await open(page, "funded");
    await openTransfer(page, "shieldedToCore");
    await page.getByTestId("transfer-max").click();
    await expect(page.getByTestId("transfer-amount")).toHaveValue(/^1\.24980*$/);
    const grants = await page.evaluate(() => window.__dwFixture!.world.grants.length);
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-quote")).toBeVisible();
    await expect(page.getByTestId("shielded-go")).toBeEnabled();
    expect(await page.evaluate(() => window.__dwFixture!.world.grants.length)).toBe(grants);
    expect(await callsOf(page, "unshield_to_core")).toBe(0);
  });

  test("an unseen result's Show on a card opens Transfer at that route's result", async ({ page }) => {
    await open(page, "funded");
    await hold(page, "platform_move_to_core");
    await openTransfer(page, "platformToCore");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    // Closed while it runs: its result waits on the card.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await release(page);
    await expect(page.getByTestId("platform-move-unseen")).toBeVisible();
    await page.getByTestId("platform-move-show").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "platformToCore");
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
  });

  test("an unseen Shielded → Platform result holds the card's Finish and Pay's Send until it is seen", async ({ page }) => {
    await open(page, "transfer-shield-pending");
    await hold(page, "unshield_to_platform");
    await openTransfer(page, "shieldedToPlatform");
    await page.getByTestId("transfer-amount").fill("0.5");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await release(page);
    await expect(page.getByTestId("shielded-unseen")).toBeVisible();
    await expect(page.getByTestId("shielded-finish")).toBeDisabled();
    // Its reason is the card's own unseen line, not a second copy of it.
    await expect(page.getByTestId("shielded-finish")).toHaveAttribute("aria-describedby", "shielded-unseen-line");
    await expect(page.getByTestId("unfinished-card").locator(".dash-button-why")).toHaveCount(0);
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByTestId("shielded-send")).toBeDisabled();
    await expect(page.getByTestId("pay-shielded")).toContainText("Your last Shielded move has an update. Open it before you send from Shielded again.");
    await page.getByTestId("sidebar-home").click();
    await page.getByTestId("shielded-show").click();
    await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "shieldedToPlatform");
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
    await page.getByTestId("shielded-done").click();
    // Seen: Finish and Send are offered again, and nothing ran in between.
    await expect(page.getByTestId("shielded-finish")).toBeEnabled();
    expect(await callsOf(page, "resume_shield")).toBe(0);
    await page.getByTestId("sidebar-pay").click();
    await expect(page.getByTestId("shielded-send")).toBeEnabled();
  });
});
