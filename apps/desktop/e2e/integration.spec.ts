import { expect, test, type Page } from "@playwright/test";
import { SECOND_WALLET_ID } from "../src/fixture/scenarios";
import { authorize, fixtureUrl, moveFromPlatform, openTransfer, platformMoveWhy } from "./helpers";

// The 0.1 integration step (E1 shielded m4 v47 + E2 Platform addresses on main): the cases the
// integration brief adds. Each test failed before its change.

async function open(page: Page, scenario = "funded", extra = "") {
  await page.goto(fixtureUrl(scenario, "light", extra));
  await page.getByTestId("hero").waitFor();
}

const world = <T,>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) => page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

const refreshPlatform = (page: Page) =>
  page.evaluate(`(() => { const f = window.__dwFixture; f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } }); })()`);

test.describe("item 2: Move waits for Activity's check", () => {
  test("after a relaunch, no Move is offered until Activity has answered, and then the pending move shows", async ({ page }) => {
    // The first Activity read of every load is held: a read still out when the card shows.
    await open(page, "funded", "&hold=platform_activity");
    await page.evaluate(`window.__dwFixture.release("platform_activity")`);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    await page.getByTestId("platform-move-done").click();
    await page.evaluate(() => window.__dwFixture!.relaunch());
    await page.getByTestId("lock-field").waitFor();
    await page.getByTestId("lock-field").fill("demo");
    await page.keyboard.press("Enter");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("platform-card")).toBeVisible();
    // Read now, not retried: the card says why, and Transfer from Platform waits with the same reason.
    expect(await page.getByTestId("platform-move-why").textContent()).toContain("Checking for an unfinished move");
    await page.getByTestId("platform-card-transfer").click();
    expect(await page.getByTestId("transfer-continue").isDisabled()).toBe(true);
    expect(await page.getByTestId("transfer-unavailable").textContent()).toContain("Checking for an unfinished move");
    await page.getByTestId("transfer-cancel").click();
    await page.evaluate(`window.__dwFixture.release("platform_activity")`);
    await expect(page.getByTestId("platform-card")).toContainText("Do not move again");
    expect(await platformMoveWhy(page)).toContain("Do not move again");
  });

  test("a failed Activity read holds Move back and can be tried again", async ({ page }) => {
    await open(page, "funded", "&fail=platform_activity:none");
    await expect(page.getByTestId("platform-move-why")).toContainText("Could not check for an unfinished move.");
    await expect(page.getByTestId("platform-move-check-again")).toBeVisible();
    expect(await platformMoveWhy(page)).toContain("Could not check for an unfinished move.");
  });

  test("a pending move that turns up while the confirmation is open replaces Confirm with View in Activity", async ({ page }) => {
    await open(page);
    await moveFromPlatform(page);
    await expect(page.getByTestId("platform-move-quote")).toBeVisible();
    await expect(page.getByTestId("platform-move-go")).toBeEnabled();
    await world(page, (w) => {
      const p = w.pending.platform!;
      p.activity = [{ id: "pm-9", kind: "moveToCore", state: "pending", credits: "40000000000", address: null, to: "y", expectedDuffs: null, feeCredits: null, at: 1, mayLand: true, estimated: false }, ...p.activity];
    });
    await refreshPlatform(page);
    await expect(page.getByTestId("platform-move-blocked")).toContainText("Do not move again");
    await expect(page.getByTestId("platform-move-go")).toHaveCount(0);
    await page.getByTestId("platform-move-blocked-view").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await expect(page.getByTestId("tx-list")).toBeVisible();
  });
});

test.describe("item 3: movable and dust, the fee actually taken", () => {
  test("Platform addresses holding only dust: Home counts nothing, and no Move is offered", async ({ page }) => {
    await open(page, "platform-dust");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.00");
    await expect(page.getByTestId("platform-card")).toContainText("cannot be moved to your Dash wallet right now");
    await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toContain("cannot be moved to your Dash wallet right now");
  });

  test("a remainder that cannot be moved is named beside the movable amount, and not counted", async ({ page }) => {
    await open(page, "platform-partly-dust");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await expect(page.getByTestId("platform-card-amount")).toContainText("0.4");
    await expect(page.getByTestId("platform-card-dust")).toContainText("0.000015 tDASH");
    await moveFromPlatform(page);
    await expect(page.getByTestId("platform-move-go")).toBeEnabled();
  });

  test("the move's outcome names the fee the engine took", async ({ page }) => {
    await open(page);
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-text")).toContainText("The network fee was 0.00002 tDASH.");
  });
});

test.describe("item 7: replaced moves, an unproved arrival, the 50-coin refusal", () => {
  test("a replaced move reads 'Replaced by a retry': no amount, and not a problem", async ({ page }) => {
    await open(page, "platform-replaced");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    const rows = page.locator('[data-txid^="platform:moveToCore:pm-old"]');
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText("Replaced by a retry");
    await expect(rows.locator(".tx-row-amount")).toHaveCount(0);
    await expect(rows).not.toContainText("Not accepted");
    await expect(rows.locator(".is-problem")).toHaveCount(0);
    // The retry that did go through keeps its amount.
    await expect(page.locator('[data-txid^="platform:moveToCore:pm-retry"] .tx-row-amount')).toHaveCount(1);
  });

  test("a move whose arrival is not proved shows what left Platform, and no 'arrives' amount", async ({ page }) => {
    await open(page);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "unproved"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    const text = page.getByTestId("platform-move-outcome-text");
    await expect(text).toContainText("left your Platform addresses");
    await expect(text).not.toContainText("on its way");
  });

  test("the 50-coin refusal shows the engine's own words", async ({ page }) => {
    await open(page);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "tooManyCoins"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toHaveText("Your Platform addresses hold too many separate balances to move at once. Consolidate coins first.");
  });
});

test.describe("item 8: closing a wallet that cannot close says why", () => {
  const closeB = async (page: Page, refusal: string) => {
    await open(page, "two-wallets");
    await page.evaluate(`window.__dwFixture.shellCommand({ kind: "navigate", route: "settings" })`);
    await page.getByTestId("settings-tabs-wallets").click();
    await page.evaluate(`window.__dwFixture.world.settings.unloadRefusal = ${JSON.stringify(refusal)}`);
    await page.getByTestId(`settings-wallet-${SECOND_WALLET_ID.slice(0, 8)}-close`).click();
  };

  test("an unresolved shielded payment: the text names it, and the wallet stays open", async ({ page }) => {
    await closeB(page, "unresolved");
    await expect(page.getByTestId("settings-wallets-error")).toHaveText("A shielded payment is still being confirmed. Wait, or quit and reopen the app.");
    await expect(page.getByTestId(`settings-wallet-${SECOND_WALLET_ID.slice(0, 8)}-state`)).not.toHaveText("Closed");
  });

  test("a wallet still opening: the engine's detail, not 'Something went wrong'", async ({ page }) => {
    await closeB(page, "opening");
    await expect(page.getByTestId("settings-wallets-error")).toHaveText("A wallet is still opening; close the network once it is open.");
  });
});

test.describe("item 9: a lock that may already be applied is 'Checking'", () => {
  test("the card says Checking with the contract's words and offers Check again, and Add is not blocked", async ({ page }) => {
    await open(page, "shielded-checking");
    const row = page.getByTestId("shielded-checking");
    await expect(row).toContainText("Checking");
    await expect(row).toContainText("This amount may already be in your shielded balance; we can't confirm it yet.");
    await expect(page.getByTestId("shielded-finish")).toHaveText("Check again");
    await expect(page.getByTestId("shielded-unfinished")).toHaveCount(0);
    // It blocks no new shield.
    await openTransfer(page, "coreToShielded");
    await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
    await page.getByTestId("transfer-amount").fill("0.001");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(row).toBeVisible();
  });

  test("Check again reads the same lock and clears it once it is proved", async ({ page }) => {
    await open(page, "shielded-checking");
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-checking")).toHaveCount(0);
  });
});

test.describe("item 10: a wallet with an unrecorded spend asks before it sends (DEC-1057)", () => {
  const toConfirm = async (page: Page, action: "send" | "out") => {
    if (action === "send") {
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("shielded-send").click();
      await page.getByTestId("shielded-amount").fill("0.5");
      await page.getByTestId("shielded-to").fill("tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2");
      await page.getByTestId("shielded-next").click();
    } else {
      await openTransfer(page, "shieldedToCore");
      await page.getByTestId("transfer-amount").fill("0.5");
      await page.getByTestId("transfer-continue").click();
    }
    await page.getByTestId("shielded-go").click();
  };

  test("the card warns, and a send needs the dialog's Send anyway, which the engine receives", async ({ page }) => {
    await open(page, "shielded-unrecorded");
    await expect(page.getByTestId("shielded-unrecorded")).toContainText("may still go through");
    await toConfirm(page, "send");
    await expect(page.getByTestId("shielded-unrecorded-dialog")).toBeVisible();
    // Going back sends nothing.
    await page.getByTestId("shielded-unrecorded-back").click();
    await expect(page.getByTestId("shielded-unrecorded-dialog")).toHaveCount(0);
    expect(await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "shielded_send").length)).toBe(0);
    await page.getByTestId("shielded-go").click();
    await page.getByTestId("shielded-unrecorded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Sent");
  });

  test("a move to the Dash wallet asks the same, and its button says Move anyway", async ({ page }) => {
    await open(page, "shielded-unrecorded");
    await toConfirm(page, "out");
    await expect(page.getByTestId("shielded-unrecorded-go")).toHaveText("Move anyway");
    await page.getByTestId("shielded-unrecorded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
  });

  test("a shield is not held back, and a send with no confirmation shows the engine's refusal text", async ({ page }) => {
    await open(page, "shielded-unrecorded");
    await openTransfer(page, "coreToShielded");
    await page.getByTestId("transfer-amount").fill("0.001");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await expect(page.getByTestId("shielded-unrecorded-dialog")).toHaveCount(0);
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
  });
});
