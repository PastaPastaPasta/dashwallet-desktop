import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl } from "./helpers";

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
    await page.getByTestId("platform-move").click();
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
    // Read now, not retried: the button is there and disabled, with its reason beside it.
    expect(await page.getByTestId("platform-move").isDisabled()).toBe(true);
    expect(await page.getByTestId("platform-move-why").textContent()).toContain("Checking for an unfinished move");
    await page.evaluate(`window.__dwFixture.release("platform_activity")`);
    await expect(page.getByTestId("platform-card")).toContainText("Do not move again");
    await expect(page.getByTestId("platform-move")).toHaveCount(0);
  });

  test("a failed Activity read holds Move back and can be tried again", async ({ page }) => {
    await open(page, "funded", "&fail=platform_activity:none");
    await expect(page.getByTestId("platform-move")).toBeDisabled();
    await expect(page.getByTestId("platform-move-why")).toContainText("Could not check for an unfinished move.");
    await expect(page.getByTestId("platform-move-check-again")).toBeVisible();
  });

  test("a pending move that turns up while the confirmation is open replaces Confirm with View in Activity", async ({ page }) => {
    await open(page);
    await page.getByTestId("platform-move").click();
    await expect(page.getByTestId("platform-move-quote")).toBeVisible();
    await expect(page.getByTestId("platform-move-go")).toBeEnabled();
    await world(page, (w) => {
      const p = w.pending.platform!;
      p.activity = [{ id: "pm-9", kind: "moveToCore", state: "pending", credits: "40000000000", address: null, to: "y", expectedDuffs: null, feeCredits: null, at: 1 }, ...p.activity];
    });
    await refreshPlatform(page);
    await expect(page.getByTestId("platform-move-blocked")).toContainText("Do not move again");
    await expect(page.getByTestId("platform-move-go")).toHaveCount(0);
    await page.getByTestId("platform-move-blocked-view").click();
    await expect(page.getByTestId("platform-move-sheet")).toHaveCount(0);
    await expect(page.getByTestId("tx-list")).toBeVisible();
  });
});

test.describe("item 3: movable and dust, the fee actually taken", () => {
  test("Platform addresses holding only dust: Home counts nothing, and no Move is offered", async ({ page }) => {
    await open(page, "platform-dust");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.00");
    await expect(page.getByTestId("platform-card")).toContainText("is too small to move");
    await expect(page.getByTestId("platform-move")).toHaveCount(0);
    await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
  });

  test("a remainder too small to move is named beside the movable amount, and not counted", async ({ page }) => {
    await open(page, "platform-partly-dust");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await expect(page.getByTestId("platform-card-amount")).toContainText("0.4");
    await expect(page.getByTestId("platform-card-dust")).toContainText("0.000015 tDASH");
    await expect(page.getByTestId("platform-move")).toBeEnabled();
  });

  test("the move's outcome names the fee the engine took", async ({ page }) => {
    await open(page);
    await page.getByTestId("platform-move").click();
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
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    const text = page.getByTestId("platform-move-outcome-text");
    await expect(text).toContainText("left your Platform addresses");
    await expect(text).not.toContainText("on its way");
  });

  test("the 50-coin refusal shows the engine's own words", async ({ page }) => {
    await open(page);
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "tooManyCoins"));
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toHaveText("Your Platform addresses hold too many separate balances to move at once. Consolidate coins first.");
  });
});
