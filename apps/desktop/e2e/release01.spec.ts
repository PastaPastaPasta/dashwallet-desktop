import { expect, test, type Locator, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, moveFromPlatform, openTransfer, platformMoveWhy, THEMES, trackPageErrors, type Theme } from "./helpers";

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
      await expect(button).toHaveText("Copy address", { timeout: 8000 });
    });
  }

  test("an icon copy button swaps to Copied and back", async ({ page }) => {
    await clipboard(page);
    await open(page, "funded");
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("shielded-receive-open").click();
    const button = page.getByRole("button", { name: "Copy Shielded address" });
    await button.click();
    await expect(button.locator(".copied-label")).toHaveText("✓ Copied");
    await expect(button.locator(".copied-label")).toHaveCount(0, { timeout: 8000 });
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

test.describe("incoming payment", () => {
  const pay = (page: Page, duffs: number) => page.evaluate(`window.__dwFixture.receivePayment(${duffs})`) as Promise<string>;
  for (const theme of THEMES) {
    test(`Receive says Received before the next address; the toast and the bell follow (${theme})`, async ({ page }) => {
      const errors = await open(page, "funded", theme);
      await page.getByTestId("sidebar-receive").click();
      const address = page.getByTestId("receive-address");
      const first = (await address.textContent())!;
      await expect(page.getByTestId("receive-received")).toHaveCount(0);
      expect(await pay(page, 999_819)).toBe(first);
      // The page says what came, on the address that was paid.
      const received = page.getByTestId("receive-received");
      await expect(received).toHaveText("✓ Received 0.00999819 tDASH");
      await expect(address).toHaveText(first);
      // App-wide toast, and the bell's badge.
      await expect(page.locator(".toast")).toHaveText("Received 0.00999819 tDASH");
      await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("1");
      await shot(page, `${theme}-received-receive-page`);
      expect(await contrast(received)).toBeGreaterThanOrEqual(4.5);
      // Then the next address.
      await expect(address).not.toHaveText(first, { timeout: 8000 });
      await expect(received).toHaveCount(0);
      // The bell lists it (no identity: its page), and opening it clears the badge.
      await page.getByTestId("toolbar-bell").click();
      await expect(page.getByTestId("bell-payment")).toContainText("Received 0.00999819 tDASH");
      await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
      await shot(page, `${theme}-received-bell`);
      expect(errors).toEqual([]);
    });
  }

  test("the toast shows on any page, and the bell popover lists the payment", async ({ page }) => {
    await open(page, "dashpay-registered");
    await page.getByTestId("hero").waitFor();
    await pay(page, 150_000_000);
    await expect(page.locator(".toast")).toHaveText("Received 1.50 tDASH");
    // The identity's three unread events and the payment.
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("4");
    await page.getByTestId("toolbar-bell").click();
    await expect(page.getByTestId("bell-popover")).toBeVisible();
    await expect(page.getByTestId("bell-payments")).toContainText("Received 1.50 tDASH");
    await expect(page.getByTestId("bell-empty")).toHaveCount(0);
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
    // A row opens the payment's detail.
    await page.getByTestId("bell-payment").click();
    await expect(page.getByTestId("tx-detail")).toBeVisible();
  });

  test("no toast over the lock screen; the bell has the payment after unlocking", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.getByTestId("toolbar-lock").click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    await pay(page, 150_000_000);
    await page.waitForTimeout(500);
    await expect(page.locator(".toast")).toHaveCount(0);
    await page.getByTestId("lock-field").fill("demo");
    await page.keyboard.press("Enter");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("1");
  });

  test("a payment to a New address says so, then moves on to the engine's address", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("sidebar-receive").click();
    const address = page.getByTestId("receive-address");
    const engines = (await address.textContent())!;
    await page.getByTestId("receive-new-address").click();
    await expect(address).not.toHaveText(engines);
    const asked = (await address.textContent())!;
    await page.evaluate(`window.__dwFixture.receivePayment(150000000, undefined, ${JSON.stringify(asked)})`);
    await expect(page.getByTestId("receive-received")).toHaveText("✓ Received 1.50 tDASH");
    await expect(address).toHaveText(asked);
    // It does not stay on the used address: the engine's current one follows.
    await expect(address).toHaveText(engines, { timeout: 8000 });
    await expect(page.getByTestId("receive-received")).toHaveCount(0);
  });

  test("Discreet mode says nothing about the amount", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.getByTestId("toolbar-discreet").click();
    await pay(page, 150_000_000);
    await expect(page.locator(".toast")).toHaveText("Received a payment");
  });
});

test("a release build has no context menu on the page, and keeps a text field's", async ({ page }) => {
  await open(page, "funded");
  await page.getByTestId("hero").waitFor();
  const prevented = (selector: string) =>
    page.evaluate((sel) => {
      const el = document.querySelector(sel)!;
      const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      el.dispatchEvent(e);
      return e.defaultPrevented;
    }, selector);
  expect(await prevented("[data-testid=hero]")).toBe(true);
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  expect(await prevented("[data-testid=pay-address]")).toBe(false);
});

test.describe("balances", () => {
  for (const theme of THEMES) {
    test(`Home shows every balance exactly, never cut to two decimals (${theme})`, async ({ page }) => {
      await open(page, "funded", theme);
      await expect(page.getByTestId("hero")).toContainText("32.04698287");
      await expect(page.getByTestId("hero")).toContainText("30.39698287");
      await expect(page.getByTestId("hero")).not.toHaveText(/(^|[^\d.])32\.04([^\d]|$)/);
      await shot(page, `${theme}-home-exact-balance`);
    });
  }
});

test.describe("first-run notice", () => {
  for (const theme of THEMES) {
    test(`is shown once, then stays dismissed (${theme})`, async ({ page }) => {
      const errors = await open(page, "pre-release-notice", theme);
      const notice = page.getByTestId("pre-release-notice");
      await expect(notice).toBeVisible();
      await expect(notice).toContainText("Dash Wallet 0.1 is an early release");
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
  // Row 74: a choice moves the running window to that network (`switch_network`), then the page
  // boots again on it. A window started with `--network` (or a flag of one network) only saves it.
  // `null` while the page boots again (no IPC yet, or the reload tore the context down).
  const activeNetwork = (page: Page) =>
    page
      .evaluate(async () => {
        const ipc = (window as unknown as { __TAURI_INTERNALS__?: { invoke(c: string, a: unknown): Promise<{ dirName: string } | null> } }).__TAURI_INTERNALS__;
        return ipc ? ((await ipc.invoke("active_network", {}))?.dirName ?? null) : null;
      })
      .catch(() => null);
  const toSettings = async (page: Page) => {
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  };
  type Knobs = NonNullable<NonNullable<Window["__dwFixture"]>["world"]["networkSwitch"]>;
  const setKnobs = (page: Page, knobs: Knobs) => page.evaluate((k) => void (window.__dwFixture!.world.networkSwitch = k), knobs);

  test("Welcome offers the choice, mainnet or testnet", async ({ page }) => {
    await open(page, "no-wallet");
    await expect(page.getByTestId("onboarding-network-testnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    await shot(page, "light-onboarding-welcome");
  });

  test("Welcome on a first run: choosing Testnet switches the window at once, and Create goes on", async ({ page }, info) => {
    await open(page, "no-wallet-mainnet");
    await expect(page.getByTestId("onboarding-network-mainnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    // The switch is held: the blocking card is up and neither action may go on.
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("switch_network"));
    const started = Date.now();
    await page.getByTestId("onboarding-network-testnet").click();
    // No wallet on this network: no confirmation.
    await expect(page.getByTestId("network-switch-sheet")).toHaveCount(0);
    await expect(page.getByTestId("network-switching")).toContainText("Opening Testnet…");
    await expect(page.getByTestId("onboarding-create")).toBeDisabled();
    await expect(page.getByTestId("onboarding-restore")).toBeDisabled();
    await shot(page, "light-onboarding-network-switching");
    await release.evaluate((r) => (r as () => void)());
    await expect.poll(() => activeNetwork(page)).toBe("testnet");
    await expect(page.getByTestId("onboarding-network-testnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    info.annotations.push({ type: "switch-ms (fixture, choice to Create enabled, held reply included)", description: String(Date.now() - started) });
    await expect(page.getByTestId("onboarding-restore")).toBeEnabled();
    await expect(page.getByTestId("network-switching")).toHaveCount(0);
    await page.getByTestId("onboarding-create").click();
    await expect(page.getByTestId("onboarding-passphrase-new")).toBeVisible();
  });

  test("measures a switch from the choice to Create enabled", async ({ page }, info) => {
    await open(page, "no-wallet-mainnet");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    const started = Date.now();
    await page.getByTestId("onboarding-network-testnet").click();
    await expect.poll(() => activeNetwork(page), { intervals: [20] }).toBe("testnet");
    await expect(page.getByTestId("onboarding-network-testnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    const ms = Date.now() - started;
    info.annotations.push({ type: "switch-ms (fixture, choice to Create enabled)", description: String(ms) });
    console.log(`row74 switch-ms ${info.project.name} ${ms}`);
  });

  test("Settings asks first on a network with a wallet, then switches the window, and back", async ({ page }) => {
    const errors = await open(page, "funded");
    await toSettings(page);
    await expect(page.getByTestId("settings-network-choice-testnet")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("settings-network-choice-mainnet").click();
    const sheet = page.getByTestId("network-switch-sheet");
    await expect(sheet).toContainText("Switch to Mainnet?");
    await expect(sheet).toContainText("Dash Wallet locks this wallet and opens Mainnet. Payments you haven't sent are dropped. Your Testnet wallet stays on this computer.");
    await expect(page.getByTestId("network-switch-pending")).toHaveCount(0);
    await shot(page, "light-settings-network-confirm");
    // Cancel changes nothing.
    await sheet.getByRole("button", { name: "Cancel" }).click();
    await expect(sheet).toHaveCount(0);
    expect(await activeNetwork(page)).toBe("testnet");
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect.poll(() => activeNetwork(page)).toBe("mainnet");
    // This data directory has no mainnet wallet: Welcome, on Mainnet, and Create goes on.
    await expect(page.getByTestId("onboarding-network-mainnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    expect(await world(page, (w) => w.pending.networkChoice)).toEqual({ chosen: "mainnet", saved: true });
    // Back to Testnet (no wallet here, so no confirmation): its wallet is there, locked.
    await page.getByTestId("onboarding-network-testnet").click();
    await expect.poll(() => activeNetwork(page)).toBe("testnet");
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("the confirmation says unfinished transfers and locks stay on the network", async ({ page }) => {
    await open(page, "recovery-one");
    await toSettings(page);
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(page.getByTestId("network-switch-pending")).toHaveText(
      "Unfinished transfers and locks stay on Testnet. You can finish them after switching back.",
    );
    await shot(page, "light-settings-network-confirm-pending");
  });

  test("a busy network refuses the switch and says what to wait for, under a picker that stays put", async ({ page }) => {
    await open(page, "funded");
    await toSettings(page);
    const mainnet = page.getByTestId("settings-network-choice-mainnet");
    // Across, where a long refusal used to push it: the card's own width, not the line's.
    const across = async () => {
      const box = await mainnet.boundingBox();
      return box && { x: box.x, width: box.width };
    };
    const before = await across();
    await setKnobs(page, { busy: { flows: ["registration", "topUp"], mixing: true, broadcast: true } });
    await mainnet.click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.getByTestId("settings-network-choice-problem")).toHaveText(
      "Wait for the payment, username registration and credit top-up to finish, or stop mixing, before switching networks.",
    );
    expect(await across()).toEqual(before);
    await expect(page.getByTestId("network-switching")).toHaveCount(0);
    await expect(page.getByTestId("settings-network-choice-testnet")).toHaveAttribute("aria-pressed", "true");
    expect(await activeNetwork(page)).toBe("testnet");
    await shot(page, "light-settings-network-busy");
    await setKnobs(page, { busy: { flows: [], mixing: true, broadcast: false } });
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.getByTestId("settings-network-choice-problem")).toHaveText("Stop mixing before switching networks.");
  });

  test("close's own refusals have their own words", async ({ page }) => {
    await open(page, "funded");
    await toSettings(page);
    await setKnobs(page, { closeRefused: "wallet" });
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.getByTestId("settings-network-choice-problem")).toHaveText("A wallet is still opening. Switch networks once it is open.");
    await setKnobs(page, { closeRefused: "shielded_spend_unresolved" });
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.getByTestId("settings-network-choice-problem")).toHaveText("A shielded payment is still being confirmed. Switch networks once it is.");
    expect(await activeNetwork(page)).toBe("testnet");
  });

  test("a switch's card covers the window: menu commands and shortcuts are ignored, and links wait", async ({ page }) => {
    await open(page, "funded");
    await toSettings(page);
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("switch_network"));
    await page.getByTestId("network-switch-confirm").click();
    const card = page.getByTestId("network-switching");
    await expect(card).toContainText("Opening Mainnet…");
    const takes = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "take_links").length);
    const taken = await takes();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.shellCommand({ kind: "navigate", route: "home" });
      f.shellCommand({ kind: "addWallet" });
      f.shellCommand({ kind: "about" });
      f.deliverLinks([{ kind: "rejected", reason: { kind: "notALink" } }]);
    });
    await page.keyboard.press("Control+2");
    await page.waitForTimeout(300);
    await expect(card).toBeVisible();
    await expect(page.getByTestId("settings-network-choice")).toBeVisible();
    await expect(page.getByTestId("page-home")).toHaveCount(0);
    await expect(page.getByTestId("onboarding-create")).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    expect(await takes()).toBe(taken);
    await release.evaluate((r) => r());
    await expect.poll(() => activeNetwork(page)).toBe("mainnet");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
  });

  // Row 74 r2: a link take asked for just before the switch, its reply arriving while the switch
  // runs. Neither applied under the card nor lost: opened once the switch is refused, and on the
  // network it was taken for only.
  const takeHeldAcrossSwitch = async (page: Page, knobs: Knobs = {}) => {
    await toSettings(page);
    await setKnobs(page, knobs);
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
    const takes = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "take_links").length);
    const taken = await takes();
    const releaseTake = await page.evaluateHandle(() => window.__dwFixture!.hold("take_links"));
    await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "scanQr" }]));
    await expect.poll(takes).toBe(taken + 1);
    const releaseSwitch = await page.evaluateHandle(() => window.__dwFixture!.hold("switch_network"));
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.getByTestId("network-switching")).toContainText("Opening Mainnet…");
    await releaseTake.evaluate((r) => r());
    await expect.poll(() => page.evaluate(() => sessionStorage.getItem("dw-links-held-by-switch"))).not.toBeNull();
    await expect(page.getByTestId("page-pay")).toHaveCount(0);
    await expect(page.getByTestId("settings-network-choice")).toBeVisible();
    await releaseSwitch.evaluate((r) => r());
  };

  test("a link taken just before a switch that is refused opens once it is", async ({ page }) => {
    await open(page, "funded");
    await takeHeldAcrossSwitch(page, { busy: { flows: [], mixing: true, broadcast: false } });
    await expect(page.getByTestId("network-switching")).toHaveCount(0);
    await expect(page.getByTestId("page-pay")).toBeVisible();
    expect(await activeNetwork(page)).toBe("testnet");
  });

  test("a link taken just before a switch that goes through is not opened on the other network", async ({ page }) => {
    await open(page, "funded");
    await takeHeldAcrossSwitch(page);
    await expect.poll(() => activeNetwork(page)).toBe("mainnet");
    await expect(page.locator(".toast")).toContainText(
      "A link that arrived during the network switch was for Testnet, so it was not opened. Open it again.",
    );
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    await expect(page.getByTestId("page-pay")).toHaveCount(0);
  });

  test("a question asked again waits for its own transfer reads, and a read that fails gives the warning", async ({ page }) => {
    trackPageErrors(page);
    await open(page, "funded");
    await toSettings(page);
    const sheet = page.getByTestId("network-switch-sheet");
    const confirm = page.getByTestId("network-switch-confirm");
    const checking = page.getByTestId("network-switch-checking");
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(confirm).toBeEnabled();
    await expect(page.getByTestId("network-switch-pending")).toHaveCount(0);
    await sheet.getByRole("button", { name: "Cancel" }).click();
    await expect(sheet).toHaveCount(0);
    // Asked again: one read is held, another fails.
    const releaseRead = await page.evaluateHandle(() => window.__dwFixture!.hold("unfinished_transfers"));
    await page.evaluate(() => window.__dwFixture!.fail("credits_pending_top_ups", "none"));
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(checking).toHaveText("Checking for unfinished transfers…");
    await expect(confirm).toBeDisabled();
    await releaseRead.evaluate((r) => r());
    await expect(page.getByTestId("network-switch-pending")).toBeVisible();
    await expect(checking).toHaveCount(0);
    await expect(confirm).toBeEnabled();
  });

  test("the question waits for the wallet list and the transfer reads before Switch works", async ({ page }) => {
    trackPageErrors(page);
    await page.goto(fixtureUrl("recovery-one", "light", "&hold=list_wallets"));
    await toSettings(page);
    await page.getByTestId("settings-network-choice-mainnet").click();
    const confirm = page.getByTestId("network-switch-confirm");
    const checking = page.getByTestId("network-switch-checking");
    const switched = () => page.evaluate(() => window.__dwFixture!.calls.includes("switch_network"));
    await expect(checking).toHaveText("Checking for unfinished transfers…");
    await expect(confirm).toBeDisabled();
    await confirm.click({ force: true });
    // The list arrives; one transfer read is still out.
    const releaseRead = await page.evaluateHandle(() => window.__dwFixture!.hold("unfinished_transfers"));
    await page.evaluate(() => window.__dwFixture!.release("list_wallets"));
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("unfinished_transfers"))).toBe(true);
    await expect(checking).toBeVisible();
    await expect(confirm).toBeDisabled();
    await confirm.click({ force: true });
    await page.waitForTimeout(200);
    expect(await switched()).toBe(false);
    await expect(page.getByTestId("network-switch-sheet")).toBeVisible();
    await releaseRead.evaluate((r) => r());
    await expect(page.getByTestId("network-switch-pending")).toBeVisible();
    await expect(checking).toHaveCount(0);
    await expect(confirm).toBeEnabled();
  });

  test("a transfer read that fails gives the warning", async ({ page }) => {
    trackPageErrors(page);
    await page.goto(fixtureUrl("funded", "light", "&fail=credits_pending_top_ups:none"));
    await toSettings(page);
    await page.getByTestId("settings-network-choice-mainnet").click();
    await expect(page.getByTestId("network-switch-pending")).toHaveText(
      "Unfinished transfers and locks stay on Testnet. You can finish them after switching back.",
    );
    await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
  });

  test("a switch that fails and rolls back boots again on the old network, with a note", async ({ page }) => {
    await open(page, "funded");
    await toSettings(page);
    await setKnobs(page, { fail: { rolledBack: true } });
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.locator(".toast")).toContainText("Mainnet did not open. Dash Wallet is back on Testnet.");
    expect(await activeNetwork(page)).toBe("testnet");
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    expect(await world(page, (w) => w.pending.networkChoice?.chosen)).toBe("testnet");
  });

  test("a switch whose way back fails too leaves the splash's Retry", async ({ page }) => {
    await open(page, "funded");
    await toSettings(page);
    await setKnobs(page, { fail: { rolledBack: false } });
    await page.getByTestId("settings-network-choice-mainnet").click();
    await page.getByTestId("network-switch-confirm").click();
    await expect(page.locator(".toast")).toContainText("Mainnet did not open, and neither did Testnet again.");
    await expect(page.getByTestId("splash").getByRole("alert")).toContainText("Dash Wallet could not open the wallet");
    await shot(page, "light-network-switch-none-open");
    await page.getByTestId("splash").getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    expect(await activeNetwork(page)).toBe("testnet");
  });

  test("a switch whose choice could not be saved says so on the new network", async ({ page }) => {
    await open(page, "no-wallet");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    await setKnobs(page, { saveFails: true });
    await page.getByTestId("onboarding-network-mainnet").click();
    await expect.poll(() => activeNetwork(page)).toBe("mainnet");
    await expect(page.locator(".toast")).toContainText("The network choice could not be saved.");
    expect(await world(page, (w) => w.pending.networkChoice?.chosen)).toBe("testnet");
  });

  test("a window started with --network only saves the choice", async ({ page }) => {
    await open(page, "network-pinned");
    await toSettings(page);
    await expect(page.getByTestId("settings-network-choice-pinned")).toContainText("--network regtest");
    await expect(page.getByTestId("settings-network-choice-mainnet")).toHaveAttribute("aria-pressed", "true");
    await page.getByTestId("settings-network-choice-testnet").click();
    // Saved for the next start, no confirmation and no switch.
    await expect(page.getByTestId("settings-network-choice-testnet")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("network-switch-sheet")).toHaveCount(0);
    expect(await world(page, (w) => w.pending.networkChoice)).toEqual({ chosen: "testnet", saved: true });
    expect(await activeNetwork(page)).toBe("regtest");
    await expect(page.getByTestId("settings-network-name")).toBeVisible();
  });

  test("a late event of the network the window left paints nothing", async ({ page }) => {
    await open(page, "no-wallet");
    await page.getByTestId("onboarding-network-mainnet").click();
    await expect.poll(() => activeNetwork(page)).toBe("mainnet");
    await expect(page.getByTestId("onboarding-create")).toBeEnabled();
    const before = await page.evaluate(() => ({ calls: window.__dwFixture!.calls.length, text: document.body.innerText }));
    // What the testnet session's last command and close would send, arriving after the reboot.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.emit({ kind: "sessionClosed", network: "testnet" });
      f.emit({ kind: "vaultLockState", network: "testnet", state: "locked" });
      f.emit({ kind: "walletCreated", network: "testnet", walletId: "late" });
      f.emit({ kind: "notice", network: "testnet", code: "spvError", detail: "late" });
    });
    await page.waitForTimeout(700);
    const after = await page.evaluate(() => ({ calls: window.__dwFixture!.calls.length, text: document.body.innerText }));
    expect(after).toEqual(before);
  });

  test("Add a wallet does not offer the network choice", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "addWallet" }));
    await expect(page.getByTestId("onboarding-create")).toBeVisible();
    await expect(page.getByTestId("onboarding-network")).toHaveCount(0);
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
    expect(saved[0].purpose).toBe("historyCsv");
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

  for (const theme of THEMES) {
    test(`the sidebar label stays readable on the selected CoinJoin item (${theme})`, async ({ page }) => {
      await open(page, "funded", theme);
      await page.getByTestId("sidebar-coinJoin").click();
      await expect(page.getByTestId("sidebar-coinJoin")).toHaveAttribute("aria-current", "page");
      const badge = page.getByTestId("sidebar-coinJoin-experimental");
      await expect(badge).toBeVisible();
      expect(await contrast(badge)).toBeGreaterThanOrEqual(4.5);
    });
  }
});

/** Transfer to Platform opens with nothing holding it back; then closed. */
async function expectFundOpens(page: Page) {
  await openTransfer(page, "coreToPlatform");
  await expect(page.getByTestId("transfer-unavailable")).toHaveCount(0);
  await expect(page.getByTestId("transfer-amount")).toBeEnabled();
  await page.getByTestId("transfer-cancel").click();
}

test.describe("Move to Dash wallet", () => {
  test("quotes, asks for approval, moves everything and shows it in Activity", async ({ page }) => {
    const errors = await open(page, "funded");
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await shot(page, "light-home-platform-card");
    await moveFromPlatform(page);
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
    await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toBe("Nothing on your Platform addresses to move yet.");
    await expectFundOpens(page);
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.00");
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
    await expect(page.getByText("Moved to Dash wallet")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("Move to Dash wallet warns when most of the balance goes to the fee", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await world(page, (w) => { w.pending.platform!.addresses = [{ address: w.pending.platform!.addresses[0].address, credits: "3000000" }]; });
    await moveFromPlatform(page);
    await expect(page.getByTestId("platform-move-fee-warning")).toHaveText("Most of this goes to fees.");
  });

  test("says so when Platform's answer did not settle it", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    await expect(page.getByTestId("platform-move-outcome-text")).toContainText("Do not move again");
    // Dismissed, it stays in Transfer: the warning, and "View in Activity" in place of "Move". Home has no card.
    await page.getByTestId("platform-move-done").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toContain("Do not move again");
    await shot(page, "light-platform-move-pending-card");
    // After a restart the card follows Activity's record.
    await page.evaluate(() => window.__dwFixture!.relaunch());
    await page.getByTestId("lock-field").fill("demo");
    await page.keyboard.press("Enter");
    await page.getByTestId("hero").waitFor();
    await expect(page.getByTestId("platform-card")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toContain("Do not move again");
    await openTransfer(page, "platformToCore");
    await page.getByTestId("platform-move-view").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await page.getByTestId("tx-list").waitFor();
    // The move settles; Home no longer holds anything back.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const p = f.world.pending.platform!;
      p.activity = p.activity.map((a) => (a.kind === "moveToCore" ? { ...a, state: "done" as const, mayLand: false } : a));
      p.addresses = [];
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } });
    });
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toBe("Nothing on your Platform addresses to move yet.");
    await expectFundOpens(page);
  });

  test("a pending move that settles while Home is shown frees the card again", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "pending"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    // Settled while the sheet is still open: it keeps showing the result.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      const p = f.world.pending.platform!;
      p.activity = p.activity.map((a) => (a.kind === "moveToCore" ? { ...a, state: "done" as const, mayLand: false } : a));
      p.addresses = [];
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "identities" } });
    });
    await expect(page.getByTestId("platform-move-outcome-title")).toHaveText("Not confirmed yet");
    await page.getByTestId("platform-move-done").click();
    await expect(page.getByTestId("platform-card-amount")).toHaveCount(0);
    expect(await platformMoveWhy(page)).toBe("Nothing on your Platform addresses to move yet.");
    await expectFundOpens(page);
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
  });

  test("shows a refusal and can be closed", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await page.evaluate(() => (window.__dwFixture!.world.pending.platform!.nextMove = "refused"));
    await moveFromPlatform(page);
    await page.getByTestId("platform-move-go").click();
    await authorize(page);
    await expect(page.getByTestId("platform-move-refusal")).toContainText("Dash Platform did not answer");
    await page.getByTestId("platform-move-close").click();
    await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
    await moveFromPlatform(page);
    await expect(page.getByTestId("platform-move-go")).toBeEnabled();
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

async function amountStep(page: Page, action: "add" | "out" | "send", amount: string) {
  if (action === "send") {
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("shielded-send").click();
    await page.getByTestId("shielded-amount").fill(amount);
    return;
  }
  await openTransfer(page, action === "add" ? "coreToShielded" : "shieldedToCore");
  await page.getByTestId("transfer-amount").fill(amount);
}

test.describe("Shielded", () => {
  test("shows Shielded in the header and Transfer in the shortcuts", async ({ page }) => {
    const errors = await open(page, "funded");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    await expect(page.getByTestId("shielded-card")).toHaveCount(0);
    await expect(page.getByTestId("shortcut-transfer")).toBeEnabled();
    await shot(page, "light-home-shielded-card");
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("Receive shows the address, its QR and a copy button", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("sidebar-receive").click();
    await page.getByTestId("shielded-receive-open").click();
    await expect(page.getByTestId("shielded-receive-address")).toContainText("tdash1zq0k8y");
    await expect(page.getByTestId("shielded-receive-qr")).toBeVisible();
    await expect(page.getByRole("button", { name: "Copy Shielded address" })).toBeVisible();
    await shot(page, "light-shielded-receive");
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("receive-select-dash").click();
    await expect(page.getByTestId("shielded-receive-qr")).toHaveCount(0);
  });

  test("Add to Shielded quotes the lock, asks for approval and grows the balance", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "add", "0.001");
    await page.getByTestId("transfer-continue").click();
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
    await page.getByTestId("transfer-continue").click();
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
    await page.getByTestId("sidebar-home").click();
    await expect(page.getByTestId("breakdown-shielded")).toContainText("0.74");
  });

  test("Send says when it is too much", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "send", "5");
    await page.getByTestId("shielded-to").fill("tdash1zabcdef");
    await page.getByTestId("shielded-next").click();
    await expect(page.getByTestId("shielded-quote-error")).toContainText("Not enough in Shielded");
  });

  test("the confirm sheet names the fixed pool fee and warns when most of the amount goes to fees", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "add", "0.0002");
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-quote")).toBeVisible();
    // The quote's own pool fee, not a constant.
    await expect(page.getByTestId("shielded-quote-pool-fee")).toHaveText("0.0001 tDASH");
    await expect(page.getByTestId("shielded-pool-fee-note")).toHaveText("The Platform pool fee is fixed at about 0.0001 tDASH, whatever you add.");
    await expect(page.getByTestId("shielded-fee-warning")).toHaveText("Most of this goes to fees.");
    await shot(page, "light-shielded-fee-warning");
    await page.getByTestId("shielded-back").click();
    await page.getByTestId("transfer-amount").fill("0.1");
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-quote")).toBeVisible();
    await expect(page.getByTestId("shielded-fee-warning")).toHaveCount(0);
    // Between a tenth and a half it says how much, not "most".
    await page.getByTestId("shielded-back").click();
    await page.getByTestId("transfer-amount").fill("0.001");
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-fee-warning")).toHaveText("Fees are 11% of this amount.");
  });

  test("Move to Dash wallet pays out, and shows in Activity", async ({ page }) => {
    await open(page, "funded");
    await amountStep(page, "out", "0.5");
    await page.getByTestId("transfer-continue").click();
    await expect(page.getByTestId("shielded-quote-receive")).toContainText("0.5");
    await page.getByTestId("shielded-go").click();
    // What the prompt says for money moving (DEC-1058 #8).
    await expect(page.getByTestId("grant-sheet")).toContainText("Enter your passphrase to authorize this payment.");
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
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-refusal")).toHaveText("Shielded withdrawals open once the network's shielded pool has enough activity. Try again later.");
    await shot(page, "light-shielded-pool-small");
  });

  for (const theme of THEMES) {
    test(`while Shielded syncs its actions wait, say why, and stay readable (${theme})`, async ({ page }) => {
      const errors = await open(page, "shielded-syncing", theme);
      await expect(page.getByTestId("breakdown-shielded")).toContainText("—");
      await expect(page.getByTestId("shielded-card")).toHaveCount(0);
      const waits = async (ids: string[]) => {
        for (const id of ids) {
          const b = page.getByTestId(id);
          await expect(b).toBeDisabled();
          await expect(b).toHaveAttribute("title", "Shielded is still syncing; try again when it finishes.");
          // Disabled is dimmed, not hidden: its text still has at least 3:1 against what is behind it.
          expect(await contrast(b)).toBeGreaterThanOrEqual(3);
        }
      };
      // Both of its Transfer routes wait and say why.
      for (const route of ["coreToShielded", "shieldedToCore"] as const) {
        await openTransfer(page, route);
        await expect(page.getByTestId("transfer-unavailable")).toHaveText("Shielded is still syncing; try again when it finishes.");
        await expect(page.getByTestId("transfer-amount")).toBeDisabled();
        await waits(["transfer-max", "transfer-continue"]);
        if (route === "shieldedToCore") await shot(page, `${theme}-shielded-syncing-disabled`);
        await page.getByTestId("transfer-cancel").click();
      }
      await page.getByTestId("sidebar-receive").click();
      await page.getByTestId("shielded-receive-open").click();
      await expect(page.getByTestId("shielded-receive-why")).toHaveText("Shielded is still syncing; try again when it finishes.");
      await page.getByTestId("sidebar-pay").click();
      await waits(["shielded-send"]);
      expect(errors).toEqual([]);
    });
  }

  test("a spend with no proved answer stays pending and is never offered again", async ({ page }) => {
    await open(page, "funded");
    await page.getByTestId("hero").waitFor();
    await shielded(page, `(s) => (s.next = "unknown")`);
    await amountStep(page, "out", "0.5");
    await page.getByTestId("transfer-continue").click();
    await page.getByTestId("shielded-go").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Not confirmed yet");
    await expect(page.getByTestId("shielded-outcome-text")).toContainText("Do not send it again");
    await expect(page.getByTestId("shielded-refused-back")).toHaveCount(0);
    await page.getByTestId("shielded-done").click();
    // Home has no card for it while the engine lists the spend: Activity shows it pending.
    await expect(page.getByTestId("hero")).toBeVisible();
    await expect(page.getByTestId("shielded-unresolved")).toHaveCount(0);
    await expect(page.getByTestId("shielded-card")).toHaveCount(0);
    await page.getByTestId("sidebar-activity").click();
    const row = page.locator('[data-txid^="shielded:withdrawal:"]').first();
    await expect(row).toContainText("Pending");
    await expect(row).not.toContainText("Not accepted");
    await expect(page.locator('button[data-txid^="shielded:"]')).toHaveCount(0);
  });

  test("shows an unavailable Shielded header balance with no Home card", async ({ page }) => {
    const errors = await open(page, "shielded-inactive");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("—");
    await expect(page.getByTestId("shielded-card")).toHaveCount(0);
    await expect(page.getByTestId("shielded-card-transfer")).toHaveCount(0);
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
