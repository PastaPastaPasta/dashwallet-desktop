import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl, openTransfer, platformMoveWhy, THEMES } from "./helpers";

/** The Home pool sections and their rows that only repeated what the sheets say. */
const POOL_CARDS = ["shielded-card", "platform-card", "platform-card-dust", "platform-card-waiting", "platform-card-send-held", "shielded-card-pending", "shielded-card-why"];

for (const theme of THEMES) {
  test(`Home has no pool cards in the normal state (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    for (const id of ["shielded-card", "platform-card", "shielded-card-text", "platform-card-amount", "shielded-card-transfer", "platform-card-transfer"]) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shortcut-transfer").click();
    await expect(page.getByTestId("transfer-sheet")).toBeVisible();
  });

  test(`Home has no pool card with only Platform dust, and Transfer says why (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("platform-dust-small", theme));
    await expect(page.getByTestId("breakdown-platform").locator(".amount-number")).toHaveText("0.00");
    for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
    await expect(page.getByTestId("page-home")).not.toContainText("cannot be moved");
    expect(await platformMoveWhy(page)).toBe("0.00400500 tDASH on your Platform addresses cannot be moved to your Dash wallet right now.");
  });

  test(`Unfinished transfers keeps both Finish paths, with no pool card (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("paddr-fund-proof-waiting", theme));
    const card = page.getByTestId("unfinished-card");
    await expect(card.getByTestId("platform-fund-unfinished")).toBeVisible();
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }];
      f.emit({ kind: "eventsOverflowed" });
    });
    await expect(card.getByTestId("shielded-unfinished")).toBeVisible();
    await expect(card.getByRole("heading")).toHaveText("Unfinished transfers");
    for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
    await page.getByTestId("shielded-finish").click();
    await authorize(page);
    await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
    await page.getByTestId("shielded-done").click();
    await expect(page.getByTestId("shielded-unfinished")).toHaveCount(0);
    await expect(card.getByTestId("platform-fund-unfinished")).toBeVisible();
    await page.getByTestId("platform-fund-finish").click();
    await authorize(page);
    await expect(page.getByTestId("platform-fund-outcome-title")).toHaveText("Added to Platform");
    await page.getByTestId("platform-fund-done").click();
    await expect(page.getByTestId("platform-fund-unfinished")).toHaveCount(0);
    await expect(card).toHaveCount(0);
  });

  for (const pool of ["shielded", "platform"] as const) {
    test(`Home retains an unseen ${pool} result and Show (${theme})`, async ({ page }) => {
      const command = pool === "shielded" ? "shield_from_core" : "platform_fund";
      const prefix = pool === "shielded" ? "shielded" : "platform-fund";
      await page.goto(fixtureUrl("funded", theme, `&hold=${command}`));
      await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
      await openTransfer(page, pool === "shielded" ? "coreToShielded" : "coreToPlatform");
      await page.getByTestId("transfer-amount").fill("0.001");
      await page.getByTestId("transfer-continue").click();
      await page.getByTestId(`${prefix}-go`).click();
      await authorize(page);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      await page.evaluate((command) => window.__dwFixture!.release(command as "shield_from_core" | "platform_fund"), command);
      await expect(page.getByTestId("unfinished-card").getByTestId(`${prefix}-unseen`)).toBeVisible();
      for (const id of POOL_CARDS) await expect(page.getByTestId(id)).toHaveCount(0);
      await page.getByTestId(`${prefix}-show`).click();
      await expect(page.getByTestId(`${prefix}-outcome-title`)).toHaveText(pool === "shielded" ? "Added to Shielded" : "Added to Platform");
    });
  }
}

test("An open Shielded passphrase prompt survives the Home section appearing and going", async ({ page }) => {
  await page.goto(fixtureUrl("funded", THEMES[0]));
  await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
  await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  await openTransfer(page, "coreToShielded");
  await page.getByTestId("transfer-amount").fill("0.001");
  await page.getByTestId("transfer-continue").click();
  await page.getByTestId("shielded-go").click();
  const input = page.getByTestId("grant-sheet").locator("input[type=password]");
  await input.fill("de");
  await input.evaluate((el) => el.setAttribute("data-kept", "1"));
  const setShields = (n: number) =>
    page.evaluate((n) => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.pendingShields = n ? [{ txid: "c0ffee00".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }] : [];
      // Not a lost event queue: that closes the wallet until the lock status is read again.
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "startup" } });
    }, n);
  await setShields(1);
  await expect(page.getByTestId("unfinished-card").getByTestId("shielded-unfinished")).toHaveCount(1);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await setShields(0);
  await expect(page.getByTestId("unfinished-card")).toHaveCount(0);
  await expect(input).toHaveAttribute("data-kept", "1");
  await expect(input).toHaveValue("de");
  await expect(input).toBeFocused();
  await input.pressSequentially("mo");
  await input.press("Enter");
  await expect(page.getByTestId("grant-sheet")).toHaveCount(0);
  await expect(page.getByTestId("shielded-outcome-title")).toHaveText("Added to Shielded");
});

for (const theme of THEMES) {
  test(`Home asks to join DashPay once, and the hero balance reads like the mobile wallets (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-no-identity", theme));
    const home = page.getByTestId("page-home");
    await expect(page.getByTestId("dashpay-card")).toHaveAttribute("data-state", "join");
    // One button on Home, the card's; the sidebar entry stays as navigation; the toolbar has none.
    await expect(home.getByRole("button", { name: "Join DashPay" })).toHaveCount(1);
    await expect(home.getByText("Join DashPay", { exact: true })).toHaveCount(1);
    await expect(page.getByTestId("toolbar-chip")).toHaveCount(0);
    await expect(page.getByTestId("sidebar-joinDashPay")).toHaveText("Join DashPay");
    await home.getByRole("button", { name: "Join DashPay" }).click();
    await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
    await page.getByTestId("sidebar-home").click();

    // The exact amount, with the decimals past the second drawn smaller.
    const number = page.getByTestId("hero-amount").locator(".amount-number");
    await expect(number).toHaveText("31.84698287");
    const minor = number.locator(".amount-minor");
    await expect(minor).toHaveText("698287");
    const size = (l: typeof number) => l.evaluate((e) => parseFloat(getComputedStyle(e).fontSize));
    expect(await size(minor)).toBeLessThan(await size(number));
    await expect(page.getByTestId("hero-amount").locator(".amount")).toHaveAttribute("title", "31.84698287 tDASH");
    // The split digits are still one spoken number.
    await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
  });
}

/** Core's balances, replaced in the world and announced as the engine would. */
const setCore = async (page: Page, over: Record<string, number>) => {
  await page.waitForFunction(() => !!window.__dwFixture);
  await page.evaluate((over) => {
    const f = window.__dwFixture!;
    const w = f.world.wallets[0];
    w.summary = { ...w.summary, balances: { ...w.summary.balances!, ...over } };
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.summary.walletId, balances: w.summary.balances });
  }, over);
};

test("Home keeps a Core-only breakdown and masks pending amounts in discreet mode", async ({ page }) => {
  await page.goto(fixtureUrl("pools-absent", "light"));
  await expect(page.getByTestId("hero-amount")).toContainText("30.19698287");
  const core = page.getByTestId("breakdown-core");
  const subtitle = core.locator(".breakdown-subtitle");
  await expect(subtitle).toHaveText("0.20 pending");
  await core.focus();
  await expect(core.getByRole("tooltip")).toHaveText("Unconfirmed, including change from your sends");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("breakdown-shielded")).toHaveCount(0);
  await expect(page.getByTestId("breakdown-platform")).toHaveCount(0);
  await page.getByTestId("toolbar-discreet").click();
  await expect(page.getByTestId("hero-amount")).toContainText("#.##");
  await expect(subtitle).toBeEmpty();
  await page.getByTestId("toolbar-discreet").click();
  await expect(subtitle).toHaveText("0.20 pending");
  await setCore(page, { confirmed: 125_000_000, unconfirmed: 0, immature: 0, locked: 0, total: 125_000_000 });
  await expect(page.getByTestId("hero-amount").locator(".amount-number")).toHaveText("1.25");
  await expect(page.getByTestId("hero-amount").locator(".amount-minor")).toHaveCount(0);
  await expect(subtitle).toBeEmpty();
});

for (const theme of THEMES) {
  test(`pending stays on its own balance type without resizing the breakdown (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await setCore(page, { unconfirmed: 0 });
    const setShielded = async (pending: boolean, balance = "0") => page.evaluate(({ pending, balance }) => {
      const f = window.__dwFixture!;
      const s = f.world.pending.shielded!;
      s.balance = balance;
      s.activity = s.activity.map((row, i) => i === 0 ? { ...row, credits: "49609190000", status: pending ? "pending" : "confirmed" } : row);
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } });
    }, { pending, balance });
    const strip = page.getByTestId("hero-breakdown");
    const core = page.getByTestId("breakdown-core");
    const shielded = page.getByTestId("breakdown-shielded");
    const platform = page.getByTestId("breakdown-platform");
    await expect(shielded).toBeVisible();
    await expect(platform.locator(".amount-number")).toHaveText("0.40");
    await setShielded(false);
    await expect(shielded.locator(".amount-number")).toHaveText("0.00");
    const hero = page.getByTestId("hero-amount");
    await expect(hero).toHaveAccessibleName("30.59698287 tDASH");
    const initialBox = await strip.boundingBox();
    expect(initialBox).not.toBeNull();

    await setCore(page, { unconfirmed: 20_000_000, withheld: 5_000_000 });
    await setShielded(true);
    await expect(core.locator(".breakdown-subtitle")).toHaveText("0.25 pending");
    await core.focus();
    await expect(core.getByRole("tooltip")).toHaveText("Unconfirmed, including change from your sends");
    await page.keyboard.press("Escape");
    await expect(shielded.locator(".breakdown-subtitle")).toBeEmpty();
    await expect(shielded.locator("[role=tooltip]")).toHaveCount(1);
    await expect(shielded.getByRole("tooltip")).toBeHidden();
    await shielded.focus();
    await expect(shielded.getByRole("tooltip")).toHaveText("Pending history (in + out): 0.4960919 tDASH");
    await page.keyboard.press("Escape");
    await expect(shielded.getByRole("tooltip")).toBeHidden();
    await expect(platform.locator(".breakdown-subtitle")).toBeEmpty();
    await expect(shielded.locator(".amount-number")).toHaveText("0.00");
    await expect(hero).toHaveAccessibleName("30.59698287 tDASH");
    expect(await strip.boundingBox()).toEqual(initialBox);

    await setCore(page, { unconfirmed: 0, withheld: 0 });
    await setShielded(false, "49609190000");
    await expect(core.locator(".breakdown-subtitle")).toBeEmpty();
    await expect(shielded.locator(".breakdown-subtitle")).toBeEmpty();
    await expect(shielded.locator(".amount-number")).toHaveText("0.4960919");
    await expect(hero).toHaveAccessibleName("31.09307477 tDASH");
    expect(await strip.boundingBox()).toEqual(initialBox);
  });

  test(`a failed Platform read keeps its row with "—" and says so (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme, "&fail=platform_addresses:none"));
    const row = page.getByTestId("breakdown-platform");
    await expect(row).toContainText("—");
    await expect(row).toContainText("not available");
    await expect(page.getByTestId("breakdown-platform-fiat")).toHaveCount(0);
    // The hero counts what is known: Core 30.19698287 + Shielded 1.25.
    await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.44698287 tDASH");
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
  });

  test(`a pool still being read keeps a placeholder row (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme, "&hold=shielded_status"));
    await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
    const row = page.getByTestId("breakdown-shielded");
    await expect(row).toContainText("…");
    await expect(row).toContainText("loading");
    await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("30.59698287 tDASH");
  });

  test(`discreet mode shows the same rows whatever the pools hold (${theme})`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await expect(page.getByTestId("breakdown-shielded")).toContainText("1.25");
    // Shielded and Platform keep their rows when empty.
    await page.evaluate(() => {
      const f = window.__dwFixture!;
      f.world.pending.shielded!.balance = "0";
      f.world.pending.platform!.addresses = [];
      f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "platformAddresses" } });
    });
    await expect(page.getByTestId("breakdown-shielded").locator(".amount-number")).toHaveText("0.00");
    await expect(page.getByTestId("breakdown-platform").locator(".amount-number")).toHaveText("0.00");
    await page.getByTestId("toolbar-discreet").click();
    for (const pool of ["core", "shielded", "platform"]) await expect(page.getByTestId(`breakdown-${pool}`)).toContainText("#.##");
    await expect(page.getByTestId("breakdown-core").locator(".breakdown-subtitle")).toBeEmpty();
    await expect(page.getByTestId("hero-breakdown")).not.toContainText("pending");
  });
}

test("a pending incoming Platform transfer leaves the reported balance intact", async ({ page }) => {
  await page.goto(fixtureUrl("paddr-fund-proof-waiting", "light"));
  const platform = page.getByTestId("breakdown-platform");
  await expect(platform.locator(".amount-number")).toHaveText("0.40");
  await expect(platform.locator(".breakdown-subtitle")).toHaveText("Incoming pending · cached balance");
  await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
});

test("Shielded pending history and held notes keep their exact amounts through settlement", async ({ page }) => {
  await page.goto(fixtureUrl("shielded-held", "light"));
  const row = page.getByTestId("breakdown-shielded");
  const subtitle = row.locator(".breakdown-subtitle");
  const strip = page.getByTestId("hero-breakdown");
  await expect(subtitle).toBeEmpty();
  await expect(row).toHaveAttribute("tabindex", "0");
  await row.focus();
  const tooltip = row.getByRole("tooltip");
  await expect(tooltip).toHaveText("0.30 tDASH held for a payment");
  const box = await strip.boundingBox();
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const s = f.world.pending.shielded!;
    s.held![0].credits = s.heldCredits = "30000000999";
    s.activity.unshift({ ...s.activity[0], id: "outgoing-pending", credits: "150", status: "pending" });
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } });
  });
  await expect(subtitle).toBeEmpty();
  await expect(tooltip).toHaveText("Pending history (in + out): 150 credits · 30,000,000,999 credits held for a payment");
  await expect(row.locator(".amount-number")).toHaveText("1.25");
  await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
  expect(await strip.boundingBox()).toEqual(box);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    const s = f.world.pending.shielded!;
    s.activity[0].status = "confirmed";
    s.held = [];
    s.heldCredits = "0";
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "shielded" } });
  });
  await expect(tooltip).toHaveCount(0);
  await expect(subtitle).toBeEmpty();
  await expect(row.locator(".amount-number")).toHaveText("1.25");
  await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
  expect(await strip.boundingBox()).toEqual(box);
});

test("a failed Platform refetch after a good read keeps the last balance", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light"));
  await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
  await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
  const reads = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "platform_addresses").length);
  const before = await reads();
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.fail("platform_addresses", "none");
    f.emit({ kind: "platform", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, change: { domain: "platformAddresses" } });
  });
  await expect.poll(reads).toBeGreaterThan(before);
  await expect(page.getByTestId("breakdown-platform")).toContainText("0.40");
  await expect(page.getByTestId("breakdown-platform")).not.toContainText("not available");
  await expect(page.getByTestId("hero-amount")).toHaveAccessibleName("31.84698287 tDASH");
});

// DP1-09 D4 reversed: the voting card's instant username while the own contest is open.
test.describe("the voting card's instant username", () => {
  const card = (page: Page) => page.getByTestId("dashpay-card");
  const action = (page: Page) => page.getByTestId("dashpay-card-action");

  test("no temporary name: the hint, and Create instant username opens the instant form", async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-contest-no-temporary", "light"));
    await expect(card(page)).toHaveAttribute("data-state", "voting");
    await expect(card(page)).toContainText("While “alice” is out for a vote, you can create an instant username to use now.");
    await expect(action(page)).toHaveText("Create instant username");
    await expect(page.getByTestId("dashpay-card-secondary")).toHaveText("What is username voting?");
    await action(page).click();
    await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
    await expect(page.getByTestId("registration-name")).toContainText("Create instant username");
  });

  test("credits short: the short line, and Top up opens the top-up sheet", async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-contest-instant-short", "light"));
    await expect(card(page)).toContainText("Not enough credits for an instant username.");
    await expect(action(page)).toHaveText("Top up");
    await action(page).click();
    await expect(page.getByTestId("page-profile")).toBeVisible();
    await expect(page.getByTestId("topup-body")).toBeVisible();
  });

  test("the registration's temporary name: iOS's success line, or its failure with Try again prefilled", async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-instant-ok", "light"));
    await expect(card(page)).toContainText("In the meantime you are reachable at “alice2”, which is yours to keep.");
    await expect(action(page)).toHaveAttribute("data-kind", "votingInfo");
    await page.goto(fixtureUrl("dashpay-instant-failed", "light"));
    await expect(card(page)).toContainText("The instant username “alice2” could not be registered, so you have no username to use while the vote runs.");
    await expect(action(page)).toHaveText("Try again");
    await action(page).click();
    await expect(page.getByTestId("registration-name-field")).toHaveValue("alice2");
  });

  test("a temporary name held: no offer", async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-contest-pending", "light"));
    await expect(card(page)).toContainText("Until then you are @alice007.");
    await expect(action(page)).toHaveAttribute("data-kind", "votingInfo");
    await expect(page.getByTestId("dashpay-card-secondary")).toHaveCount(0);
  });
});
