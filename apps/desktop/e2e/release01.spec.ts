import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// The 0.1 release's renderer pieces on the fixture backend: the first-run notice, the network
// choice, Export CSV, Show Masternodes, the Experimental label and Home's three-pool balance.
// The rules are unit-tested (network/choice, format/homeBalance, history/exportCsv); here the
// screens drive them. DW_SHOTS=<dir> also writes a screenshot of each state.

const world = <T,>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function shot(page: Page, name: string) {
  const dir = process.env.DW_SHOTS;
  if (dir) {
    await fontsReady(page);
    await page.screenshot({ path: `${dir}/${name}.png` });
  }
}

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  return errors;
}

test.describe("first-run notice", () => {
  for (const theme of THEMES) {
    test(`is shown once, then stays dismissed (${theme})`, async ({ page }) => {
      const errors = await open(page, "pre-release-notice", theme);
      const notice = page.getByTestId("pre-release-notice");
      await expect(notice).toBeVisible();
      await expect(notice).toContainText("Dash Wallet 0.1 is a pre-release");
      await expect(notice).toContainText("It has had no security review; use small amounts.");
      await expect(page.getByTestId("pre-release-dismiss")).toBeFocused();
      expect(await seriousAxeViolations(page)).toEqual([]);
      await shot(page, `${theme}-pre-release-notice`);
      await page.getByTestId("pre-release-dismiss").click();
      await expect(notice).toBeHidden();
      await expect.poll(() => world(page, (w) => w.settings.ui.preReleaseNoticeDismissed)).toBe(true);
      await expect(page.getByTestId("hero")).toBeVisible();
      expect(errors).toEqual([]);
    });
  }

  test("covers the Welcome screen: it keeps focus, and Welcome is inert until dismissed", async ({ page }) => {
    await open(page, "no-wallet-first-run");
    await expect(page.getByTestId("pre-release-notice")).toBeVisible();
    await expect(page.getByTestId("pre-release-dismiss")).toBeFocused();
    await expect(page.getByTestId("onboarding")).toHaveAttribute("inert", "");
    await page.getByTestId("pre-release-dismiss").click();
    await expect(page.getByTestId("onboarding")).not.toHaveAttribute("inert", "");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
  });

  test("covers the lock screen: the passphrase field takes no focus", async ({ page }) => {
    await open(page, "locked-first-run");
    await expect(page.getByTestId("pre-release-notice")).toBeVisible();
    await expect(page.getByTestId("lock-screen")).toHaveAttribute("inert", "");
    await expect(page.getByTestId("lock-field")).not.toBeFocused();
    await expect(page.getByTestId("pre-release-dismiss")).toBeFocused();
  });

  test("is not shown once it was dismissed", async ({ page }) => {
    await open(page, "funded");
    await expect(page.getByTestId("hero")).toBeVisible();
    await expect(page.getByTestId("pre-release-notice")).toHaveCount(0);
  });
});

test.describe("network choice", () => {
  test("Welcome waits for the restart when the choice is not this window's network", async ({ page }) => {
    await open(page, "no-wallet-network-restart");
    await expect(page.getByTestId("onboarding-network-restart")).toContainText("Restart Dash Wallet to use Mainnet.");
    await expect(page.getByTestId("onboarding-network-mainnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeDisabled();
    await expect(page.getByTestId("onboarding-restore")).toBeDisabled();
    await shot(page, "light-onboarding-network-restart");
    await page.getByTestId("onboarding-network-testnet").click();
    await expect(page.getByTestId("onboarding-network-restart")).toHaveCount(0);
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    expect(await world(page, (w) => w.pending.networkChoice?.chosen)).toBe("testnet");
  });

  test("Welcome offers the choice, mainnet or testnet", async ({ page }) => {
    await open(page, "no-wallet");
    await expect(page.getByTestId("onboarding-network-testnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    await shot(page, "light-onboarding-welcome");
  });

  test("Settings saves the choice and says a restart applies it", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    const picker = page.getByTestId("settings-network-choice");
    await expect(picker).toBeVisible();
    await expect(page.getByTestId("settings-network-choice-restart")).toHaveCount(0);
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(page.getByTestId("settings-network-choice-restart")).toContainText("Restart Dash Wallet to use Mainnet.");
    expect(await world(page, (w) => w.pending.networkChoice?.chosen)).toBe("mainnet");
    await shot(page, "light-settings-network");
    await page.getByTestId("settings-network-choice-testnet").click();
    await expect(page.getByTestId("settings-network-choice-restart")).toHaveCount(0);
  });
});

test.describe("Export CSV", () => {
  test("saves the history through the file dialog", async ({ page }) => {
    const errors = await open(page, "funded");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await shot(page, "light-activity-export");
    await page.getByTestId("tx-export-csv").click();
    await expect(page.getByTestId("tx-export-result")).toContainText("Exported to ");
    const saved = await world(page, (w) => w.tools.files.saved);
    expect(saved).toHaveLength(1);
    expect(saved[0].purpose).toBe("csv");
    expect(saved[0].path).toMatch(/Dash Wallet history \d{4}-\d{2}-\d{2}\.csv$/);
    const csv = await page.evaluate((b64) => atob(b64), saved[0].data);
    expect(csv.split("\n")[0]).toBe('"Confirmed","Date","Type","Label","Address","Amount (DASH)","ID"');
    expect(csv.split("\n").length).toBeGreaterThan(5);
    expect(errors).toEqual([]);
  });

  test("exports only what the filters select", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await page.getByTestId("tx-export-csv").click();
    await expect(page.getByTestId("tx-export-result")).toBeVisible();
    await page.getByTestId("tx-category-received").click();
    await expect(page.getByTestId("tx-footer")).toBeVisible();
    await page.getByTestId("tx-export-csv").click();
    await expect.poll(() => world(page, (w) => w.tools.files.saved.length)).toBe(2);
    const [all, received] = await world(page, (w) => w.tools.files.saved.map((f) => f.data));
    const lines = (b64: string) => page.evaluate((s) => atob(s).split("\n").length, b64);
    expect(await lines(received)).toBeLessThan(await lines(all));
  });

  test("a cancelled dialog says nothing", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.tools.files.saveDir = null));
    await page.getByTestId("tx-export-csv").click();
    await expect(page.getByTestId("tx-export-csv")).toBeEnabled();
    await expect(page.getByTestId("tx-export-result")).toHaveCount(0);
    expect(await world(page, (w) => w.tools.files.dialogs.length)).toBe(1);
    expect(await world(page, (w) => w.tools.files.saved.length)).toBe(0);
  });
});

test.describe("Show Masternodes", () => {
  test("hides Masternodes until the setting is on, and sends the route home", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("sidebar-masternodes")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "masternodes" }));
    await expect(page.getByTestId("page-masternodes")).toHaveCount(0);
    await expect(page.getByTestId("page-home")).toBeVisible();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await page.getByTestId("settings-show-masternodes").check();
    await page.getByTestId("settings-governance-turn-on").click();
    await expect(page.getByTestId("sidebar-masternodes")).toBeVisible();
    await page.getByTestId("settings-show-masternodes").uncheck();
    await expect(page.getByTestId("sidebar-masternodes")).toHaveCount(0);
  });

  test("a proposal in progress does not show it", async ({ page }) => {
    // `governance`: the setting is on and one of the wallet's proposals is confirming.
    await open(page, "governance");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("sidebar-masternodes")).toBeVisible();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await page.getByTestId("settings-show-masternodes").uncheck();
    await expect(page.getByTestId("sidebar-masternodes")).toHaveCount(0);
    await expect(page.getByTestId("status-governance")).toHaveCount(0);
  });
});

test.describe("CoinJoin is experimental", () => {
  test("is labelled in the sidebar, on its page and in its settings", async ({ page }) => {
    await open(page, "funded");
    await expect(page.getByTestId("sidebar-coinJoin-experimental")).toHaveText("Experimental");
    await shot(page, "light-sidebar-experimental");
    await page.getByTestId("sidebar-coinJoin").click();
    await expect(page.getByTestId("coinjoin-experimental")).toBeVisible();
    await shot(page, "light-coinjoin-experimental");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await expect(page.getByTestId("settings-tabs-coinjoin")).toHaveText("CoinJoin (experimental)");
  });
});

test.describe("Move to Dash wallet", () => {
  test("quotes, asks for approval, moves everything and shows it in Activity", async ({ page }) => {
    const errors = await open(page, "funded");
    await expect(page.getByTestId("platform-card-amount")).toContainText("0.4");
    await shot(page, "light-home-platform-card");
    await page.getByTestId("platform-move").click();
    await expect(page.getByTestId("platform-move-quote")).toBeVisible();
    await expect(page.getByTestId("platform-move-quote-duffs")).toContainText("0.39998");
    await expect(page.getByTestId("platform-move-quote-to")).toHaveText(/^y/);
    await shot(page, "light-platform-move-quote");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("platform-move-go").click();
    await expect(page.getByTestId("grant-sheet")).toBeVisible();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Moved");
    await shot(page, "light-platform-move-done");
    await page.getByTestId("platform-move-done").click();
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.00");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Moved to Dash wallet")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("says so when Platform's answer did not settle it", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    await expect(page.getByTestId("platform-move-outcome-text")).toContainText("Do not move again");
  });

  test("shows a refusal and can be closed", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "refused"));
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toContainText("Dash Platform did not answer");
    await page.getByTestId("platform-move-close").click();
    await expect(page.getByTestId("platform-move-sheet")).toHaveCount(0);
    await expect(page.getByTestId("platform-move")).toBeEnabled();
  });

  test("offers nothing without Platform funds, or without the command", async ({ page }) => {
    await open(page, "pools-absent");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
  });
});

test.describe("Platform activity in Activity", () => {
  test("mixes Platform rows into the list with a tag, and they open nothing", async ({ page }) => {
    const errors = await open(page, "funded");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    const rows = page.locator('[data-txid^="platform:"]');
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toContainText("Received on Platform");
    await expect(rows.first()).toContainText("Platform");
    await expect(page.locator('button[data-txid^="platform:"]')).toHaveCount(0);
    await shot(page, "light-activity-platform");
    expect(await seriousAxeViolations(page)).toEqual([]);
    // Only under "All": a type filter is the Core engine's.
    await page.getByTestId("tx-category-sent").click();
    await expect(page.locator('[data-txid^="platform:"]')).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test("leaves them out when the command is absent", async ({ page }) => {
    await open(page, "pools-absent");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.locator('[data-txid^="platform:"]')).toHaveCount(0);
  });
});

const shielded = (page: Page, fn: string) => page.evaluate(`(${fn})(window.__dwFixture.world.pending.shielded)`);

async function amountStep(page: Page, action: string, amount: string) {
  await page.getByTestId(`shielded-${action}`).click();
  await page.getByTestId("shielded-amount").fill(amount);
}

test.describe("Shielded", () => {
  test("shows the card with its balance and actions", async ({ page }) => {
    const errors = await open(page, "funded");
    await expect(page.getByTestId("shielded-card")).toBeVisible();
    for (const a of ["receive-open", "send", "add", "out"]) await expect(page.getByTestId(`shielded-${a}`)).toBeEnabled();
    await shot(page, "light-home-shielded-card");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("Receive shows the address, its QR and a copy button", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("shielded-receive-open").click();
    await expect(page.getByTestId("shielded-receive-address")).toContainText("tdash1zq0k8y");
    await expect(page.getByTestId("shielded-receive-qr")).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy Shielded address" })).toBeVisible();
    await shot(page, "light-shielded-receive");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("shielded-receive-close").click();
    await expect(page.getByTestId("shielded-receive-sheet")).toHaveCount(0);
  });

  test("Add to Shielded quotes the lock, asks for approval and grows the balance", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "add", "0.001");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-total")).toContainText("0.00101");
    await expect(page.getByTestId("shielded-quote-gain")).toContainText("0.0009 ");
    await shot(page, "light-shielded-add-quote");
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    // 1.25 plus the lock less the pool's fee, in credits.
    await expect.poll(() => shielded(page, `(s) => s.balance`)).toBe("125090000000");
  });

  test("a lock that is not applied is finished from the card", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await shielded(page, `(s) => (s.shield = "proofWaiting")`);
    await amountStep(page, "add", "0.001");
    await page.getByTestId("shielded-next").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Locked, not applied yet");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-unfinished")).toContainText("not applied yet");
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-unfinished")).toHaveCount(0);
  });

  test("Send tells the proof takes a while, refuses a bad address and sends", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "send", "0.5");
    await page.getByTestId("shielded-to").fill("yNotAShieldedAddress");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-error")).toHaveText("Enter a Shielded address of this network.");
    await page.getByTestId("shielded-back").click();
    await page.getByTestId("shielded-to").fill("tdash1zabcdef");
    await page.getByTestId("shielded-memo").fill("Lunch");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-to")).toHaveText("tdash1zabcdef");
    await expect(page.getByTestId("shielded-slow")).toContainText("Making the private proof takes up to a minute");
    await shot(page, "light-shielded-send-quote");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Sent");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("breakdown-shielded")).toContainText("0.74");
  });

  test("Send says when it is too much", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "send", "5");
    await page.getByTestId("shielded-to").fill("tdash1zabcdef");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-error")).toContainText("Not enough in Shielded");
  });

  test("Move to Dash wallet pays out, and shows in Activity", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "out", "0.5");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-receive")).toContainText("0.5");
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Moved");
    await page.getByTestId("shielded-done").click();
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.locator('[data-txid^="shielded:unshield:"]')).toContainText("Moved to Dash wallet");
    await expect(page.locator('[data-txid^="shielded:unshield:"]')).toContainText("Shielded");
  });

  test("Move to Dash wallet says when the pool is too small", async ({ page }) => {
    await open(page, "shielded-pool-small");
    await amountStep(page, "out", "0.5");
    await page.getByTestId("shielded-next").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-refusal")).toHaveText("Shielded withdrawals open once the network's shielded pool has enough activity. Try again later.");
    await shot(page, "light-shielded-pool-small");
  });

  test("a spend with no proved answer says it may have been sent", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await shielded(page, `(s) => (s.next = "unknown")`);
    await amountStep(page, "out", "0.5");
    await page.getByTestId("shielded-next").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-refusal")).toContainText("It may have been sent");
    await expect(page.getByTestId("shielded-refused-back")).toHaveCount(0);
  });

  test("says Shielded is not active on this network yet, with no actions", async ({ page }) => {
    const errors = await open(page, "shielded-inactive");
    await expect(page.getByTestId("shielded-card-text")).toHaveText("Shielded is not active on this network yet.");
    await expect(page.getByTestId("shielded-send")).toHaveCount(0);
    await expect(page.getByTestId("breakdown-shielded")).toContainText("—");
    await shot(page, "light-shielded-inactive");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("is not on Home while the commands are absent", async ({ page }) => {
    await open(page, "pools-absent");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("shielded-card")).toHaveCount(0);
  });

  test("Activity lists the shielded rows with a Shielded tag, that open nothing", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    const rows = page.locator('[data-txid^="shielded:"]');
    await expect(rows).toHaveCount(3);
    await expect(page.locator('[data-txid^="shielded:received:"]')).toContainText("Received in Shielded");
    await expect(page.locator('[data-txid^="shielded:shieldFromCore:"]')).toContainText("Added to Shielded");
    await expect(page.locator('button[data-txid^="shielded:"]')).toHaveCount(0);
  });
});

test.describe("Home balance", () => {
  for (const theme of THEMES) {
    test(`shows Core, Shielded and Platform and a hero that adds them up (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await expect(page.getByTestId("breakdown-core")).toContainText("Core");
      await expect(page.getByTestId("breakdown-shielded")).toContainText("Shielded");
      await expect(page.getByTestId("breakdown-platform")).toContainText("Platform");
      // Core 30.39698287 + Shielded 1.25 + Platform 0.4 (the fixture's), two decimals floored.
      await expect(page.getByTestId("hero-amount")).toContainText("32.04");
      await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
      await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
      await shot(page, `${theme}-home-pools`);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test("shows — for a pool without a command, and leaves it out of the hero", async ({ page }) => {
    await open(page, "pools-absent");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("—");
    await expect(page.getByTestId("breakdown-platform")).toContainText("—");
    await expect(page.getByTestId("breakdown-platform")).toBeVisible();
    await expect(page.getByTestId("hero-amount")).toContainText("30.39");
    await shot(page, "light-home-pools-absent");
  });

  test("masks all three in discreet mode", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("toolbar-discreet").click();
    for (const pool of ["core", "shielded", "platform"]) await expect(page.getByTestId(`breakdown-${pool}`)).toContainText("#.##");
  });
});
