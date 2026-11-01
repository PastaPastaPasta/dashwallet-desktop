import { expect, test, type Locator, type Page } from "@playwright/test";
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

/** The contrast ratio of an element's text against the first opaque background behind it. */
async function contrast(el: Locator): Promise<number> {
  return el.evaluate((node) => {
    const parse = (c: string): [number, number, number, number] => {
      const m = c.match(/-?[\d.]+/g)!.map(Number);
      if (c.startsWith("color(")) return [m[0] * 255, m[1] * 255, m[2] * 255, m[3] ?? 1];
      return [m[0], m[1], m[2], m[3] ?? 1];
    };
    const over = (top: number[], bottom: number[]) => {
      const a = top[3];
      return [0, 1, 2].map((i) => top[i] * a + bottom[i] * (1 - a)).concat(1);
    };
    const layers: number[][] = [];
    for (let n: Element | null = node; n; n = n.parentElement) {
      const bg = parse(getComputedStyle(n).backgroundColor);
      if (bg[3] > 0) layers.push(bg);
      if (bg[3] === 1) break;
    }
    let base: number[] = [255, 255, 255, 1];
    for (const l of layers.reverse()) base = over(l, base);
    const fg = over(parse(getComputedStyle(node).color), base);
    const lum = (c: number[]) => {
      const [r, g, b] = c.slice(0, 3).map((v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const [hi, lo] = [lum(fg), lum(base)].sort((a, b) => b - a);
    return (hi + 0.05) / (lo + 0.05);
  });
}

async function open(page: Page, scenario: string, theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  return errors;
}

test.describe("copy feedback", () => {
  const clipboard = (page: Page) =>
    page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) } });
    });
  for (const theme of THEMES) {
    test(`the button says Copied for a moment, and the toast stays (${theme})`, async ({ page }) => {
      await clipboard(page);
      await open(page, "funded", theme);
      await page.getByTestId("sidebar-receive").click();
      const button = page.getByTestId("receive-copy-address");
      await expect(button).toHaveText("Copy address");
      await button.click();
      await expect(button).toHaveText("✓ Copied");
      await expect(page.locator(".toast")).toHaveText("Copied");
      await shot(page, `${theme}-copy-button-copied`);
      expect(await contrast(button.locator(".copied-label"))).toBeGreaterThanOrEqual(3);
      // About 1.5 s later it is the label again.
      await expect(button).toHaveText("Copy address", { timeout: 4000 });
    });
  }

  test("an icon copy button swaps to Copied and back", async ({ page }) => {
    await clipboard(page);
    await open(page, "funded");
    await page.getByTestId("shielded-receive-open").click();
    const button = page.getByRole("button", { name: "Copy Shielded address" });
    await button.click();
    await expect(page.getByRole("button", { name: "Copied" })).toHaveText("✓ Copied");
    await expect(page.getByRole("button", { name: "Copy Shielded address" })).toBeVisible({ timeout: 4000 });
  });
});

test.describe("status bar", () => {
  for (const theme of THEMES) {
    test(`a new block while synced stays Synced, never Syncing 100% (${theme})`, async ({ page }) => {
      await open(page, "funded", theme);
      const bar = page.getByTestId("status-bar");
      await expect(bar).toContainText("Up to date");
      // The engine's reading for a new block: not caught up, every phase one block behind.
      await page.evaluate(`(() => {
        const f = window.__dwFixture;
        const s = f.world.sync;
        const tip = s.tipHeight + 1;
        f.world.sync = { ...s, caughtUp: false, activePhase: "headers", phases: s.phases.map((p) => ({ ...p, targetHeight: tip, done: false })) };
        f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
      })()`);
      await page.waitForTimeout(600);
      await expect(bar).toContainText("Up to date");
      await expect(bar).toContainText("Synced");
      await expect(bar).not.toContainText("Syncing");
      await expect(bar.locator("progress")).toHaveCount(0);
      await shot(page, `${theme}-status-bar-synced-after-block`);
    });
  }
});

test.describe("sidebar", () => {
  for (const theme of THEMES) {
    test(`Settings is the last item of the sidebar and opens Settings (${theme})`, async ({ page }) => {
      await open(page, "funded", theme);
      const settings = page.getByTestId("sidebar-settings");
      await expect(settings).toHaveText("Settings");
      // Last in the order and below the other items.
      const last = page.getByTestId("sidebar").getByRole("button").last();
      await expect(last).toHaveAttribute("data-testid", "sidebar-settings");
      const [a, b] = await Promise.all([page.getByTestId("sidebar-activity").boundingBox(), settings.boundingBox()]);
      expect(b!.y).toBeGreaterThan(a!.y);
      await shot(page, `${theme}-sidebar-settings`);
      await settings.click();
      await expect(page.getByTestId("page-settings")).toBeVisible();
      await expect(settings).toHaveAttribute("aria-current", "page");
      await shot(page, `${theme}-sidebar-settings-open`);
    });
  }
});

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

  test("Welcome waits while a choice is being saved, then for the saved view", async ({ page }) => {
    await open(page, "no-wallet");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    // The save's reply is late: the picker still shows the old choice, so neither action may go on.
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("set_network_choice"));
    await page.getByTestId("onboarding-network-mainnet").click();
    await expect(page.getByTestId("onboarding-create")).toBeDisabled();
    await expect(page.getByTestId("onboarding-restore")).toBeDisabled();
    await expect(page.getByTestId("onboarding-network-restart")).toHaveCount(0);
    await release.evaluate((r) => (r as () => void)());
    // The saved view says this window is on the wrong network: still not.
    await expect(page.getByTestId("onboarding-network-restart")).toContainText("Restart Dash Wallet to use Mainnet.");
    await expect(page.getByTestId("onboarding-create")).toBeDisabled();
    await expect(page.getByTestId("onboarding-restore")).toBeDisabled();
    // Back to this window's network (a held save again): it goes on once the saved view allows it.
    const release2 = await page.evaluateHandle(() => window.__dwFixture!.hold("set_network_choice"));
    await page.getByTestId("onboarding-network-testnet").click();
    await expect(page.getByTestId("onboarding-create")).toBeDisabled();
    await release2.evaluate((r) => (r as () => void)());
    await expect(page.getByTestId("onboarding-network-restart")).toHaveCount(0);
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    await expect(page.getByTestId("onboarding-restore")).toBeEnabled();
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
    // Dismissed, it stays: the warning, and "View in Activity" in place of "Move".
    await page.getByTestId("platform-move-done").click();
    await expect(page.getByTestId("platform-move-sheet")).toHaveCount(0);
    await expect(page.getByTestId("platform-card")).toContainText("Do not move again");
    await expect(page.getByTestId("platform-move")).toHaveCount(0);
    await shot(page, "light-platform-move-pending-card");
    // After a restart the card follows Activity's record.
    await page.evaluate(() => window.__dwFixture!.relaunch());
    await page.getByTestId("lock-field").fill("demo");
    await page.keyboard.press("Enter");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("platform-card")).toContainText("Do not move again");
    await expect(page.getByTestId("platform-move")).toHaveCount(0);
    await page.getByTestId("platform-move-view").click();
    await page.getByTestId("tx-list").waitFor();
    // The move settles; Home no longer holds anything back.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const p = f.world.pending.platform!;
      p.activity = p.activity.map((a) => (a.kind === "moveToCore" ? { ...a, state: "done" as const } : a));
      p.addresses = [];
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } });
    });
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
  });

  test("a pending move that settles while Home is shown frees the card again", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await page.getByTestId("platform-move").click();
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    // Settled while the sheet is still open: it keeps showing the result.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const p = f.world.pending.platform!;
      p.activity = p.activity.map((a) => (a.kind === "moveToCore" ? { ...a, state: "done" as const } : a));
      p.addresses = [];
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } });
    });
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    await page.getByTestId("platform-move-done").click();
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
    await expect(page.getByTestId("platform-move-sheet")).toHaveCount(0);
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
    // Every fee the quote reports is shown, and the debit reconciles with the credit.
    await expect(page.getByTestId("shielded-quote-fee")).toContainText("0.00001 ");
    await expect(page.getByTestId("shielded-quote-pool-fee")).toContainText("0.0001 ");
    const amountOf = async (id: string) => Math.round(parseFloat(((await page.getByTestId(id).textContent()) ?? "").replace(/[^0-9.]/g, "")) * 1e8);
    expect(await amountOf("shielded-quote-total")).toBe(
      (await amountOf("shielded-quote-fee")) + (await amountOf("shielded-quote-pool-fee")) + (await amountOf("shielded-quote-gain")),
    );
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
    await expect(page.locator('[data-txid^="shielded:withdrawal:"]')).toContainText("Moved to Dash wallet");
    await expect(page.locator('[data-txid^="shielded:withdrawal:"]')).toContainText("Shielded");
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

  for (const theme of THEMES) {
    test(`while Shielded syncs its actions wait, say why, and stay readable (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-syncing", theme);
      const card = page.getByTestId("shielded-card");
      await expect(card).toContainText("Shielded is still syncing; try again when it finishes.");
      for (const id of ["shielded-receive-open", "shielded-send", "shielded-add", "shielded-out"]) {
        const b = page.getByTestId(id);
        await expect(b).toBeDisabled();
        await expect(b).toHaveAttribute("title", "Shielded is still syncing; try again when it finishes.");
        // Disabled is dimmed, not hidden: its text still has at least 3:1 against what is behind it.
        expect(await contrast(b)).toBeGreaterThanOrEqual(3);
      }
      await shot(page, `${theme}-shielded-syncing-disabled`);
      expect(errors).toEqual([]);
    });
  }

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
      // Core 30.39698287 + Shielded 1.25 + Platform 0.4 (the fixture's), the exact sum.
      await expect(page.getByTestId("hero-amount")).toContainText("32.04698287");
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
    await expect(page.getByTestId("hero-amount")).toContainText("30.39698287");
    await shot(page, "light-home-pools-absent");
  });

  test("masks all three in discreet mode", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("toolbar-discreet").click();
    for (const pool of ["core", "shielded", "platform"]) await expect(page.getByTestId(`breakdown-${pool}`)).toContainText("#.##");
  });
});
