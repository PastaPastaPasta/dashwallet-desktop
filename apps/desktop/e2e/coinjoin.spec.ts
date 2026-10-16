import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { STATES } from "./coinjoin";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// U-06: the CoinJoin Home card, the CoinJoin page and Settings ▸ CoinJoin on the fixture backend
// (docs/design/U-06-coinjoin.md): every state, Start / Stop Mixing, the level, a lock while
// mixing, the settings form, failures; 0 serious axe issues in light and dark.

async function open(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const navigate = (page: Page, route: "coinJoin" | "home" | "settings") =>
  page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);

async function openPage(page: Page, scenario: string, theme: Theme = "light", extra = "") {
  const errors = await open(page, scenario, theme, extra);
  await navigate(page, "coinJoin");
  await expect(page.getByTestId("page-coinJoin")).toBeVisible();
  return errors;
}

async function openSettings(page: Page, scenario: string, theme: Theme = "light") {
  const errors = await open(page, scenario, theme);
  await navigate(page, "settings");
  await page.getByTestId("settings-tabs-coinjoin").click();
  await expect(page.getByTestId("settings-coinjoin")).toBeVisible();
  return errors;
}

const status = (page: Page) => page.getByTestId("coinjoin-status");
const card = (page: Page) => page.getByTestId("coinjoin-card");
const calls = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "start_mixing" || c === "stop_mixing"));

/** What the engine's refresher would do: the mixer moves on, and `coinJoin` says so. */
const mixerMoves = (page: Page, patch: Record<string, unknown>) =>
  page.evaluate(
    ({ walletId, patch }) => {
      const f = window.__dwFixture!;
      Object.assign(f.world.coinjoin.mixers[walletId], patch);
      f.emit({ kind: "coinJoin", network: f.world.launch.network.dirName, walletId });
    },
    { walletId: FIXTURE_WALLET_ID, patch },
  );

for (const theme of THEMES) {
  for (const [state, { scenario, extra, title, home }] of Object.entries(STATES)) {
    test(`U-06 ${state} ${theme}: page and Home card, 0 serious axe issues`, async ({ page }) => {
      const errors = await open(page, scenario, theme, extra);
      await expect(page.getByTestId("history-header")).toBeVisible();
      if (home) {
        await expect(card(page)).toHaveAttribute("data-state", state);
        await expect(card(page).locator("h2")).toHaveText(title);
        await fontsReady(page);
        expect(await seriousAxeViolations(page)).toEqual([]);
      } else await expect(card(page)).toHaveCount(0);
      await navigate(page, "coinJoin");
      await expect(status(page)).toHaveAttribute("data-state", state);
      await expect(page.getByTestId("coinjoin-status-title")).toHaveText(title);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test(`U-06 Settings ▸ CoinJoin and Session details ${theme}: 0 serious axe issues`, async ({ page }) => {
    const errors = await openPage(page, "coinjoin-mixing", theme);
    await page.getByTestId("coinjoin-details").locator("summary").click();
    await expect(page.getByTestId("coinjoin-sessions").locator("tbody tr")).toHaveCount(1);
    await fontsReady(page);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await navigate(page, "settings");
    await page.getByTestId("settings-tabs-coinjoin").click();
    await expect(page.getByTestId("settings-coinjoin-mixing")).toBeVisible();
    expect(await seriousAxeViolations(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("acceptance: mixing starts and stops from the page", async ({ page }) => {
  const errors = await openPage(page, "funded");
  await expect(status(page)).toHaveAttribute("data-state", "off");
  await expect(page.getByTestId("coinjoin-action")).toHaveText("Start Mixing");
  await page.getByTestId("coinjoin-action").click();
  await expect(status(page)).toHaveAttribute("data-state", "starting");
  await expect(page.getByTestId("coinjoin-action")).toHaveText("Stop Mixing");
  // The first step runs, then the mixer joins a queue.
  await mixerMoves(page, { status: { kind: "waitingInQueue" }, progress: { overallPercent: 12.5, denominatedPercent: 40, partiallyMixedPercent: 10, mixedPercent: 5, averageRounds: 0.4 } });
  await expect(status(page)).toHaveAttribute("data-state", "mixing");
  await expect(page.getByTestId("coinjoin-status-percent")).toHaveText("12%");
  await expect(page.getByTestId("coinjoin-status-line")).toHaveText("Submitted to masternode, waiting in queue .");
  // Stop asks first (D8); Cancel keeps mixing.
  await page.getByTestId("coinjoin-action").click();
  await expect(page.getByTestId("coinjoin-stop-sheet")).toBeVisible();
  await page.getByTestId("coinjoin-stop-sheet").getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByTestId("coinjoin-stop-sheet")).toHaveCount(0);
  await expect(status(page)).toHaveAttribute("data-state", "mixing");
  await page.getByTestId("coinjoin-action").click();
  await page.getByTestId("coinjoin-stop-confirm").click();
  await expect(status(page)).toHaveAttribute("data-state", "off");
  expect(await calls(page)).toEqual(["start_mixing", "stop_mixing"]);
  expect(await page.evaluate((w) => window.__dwFixture!.world.coinjoin.mixers[w].stopReason, FIXTURE_WALLET_ID)).toBe("userRequested");
  expect(errors).toEqual([]);
});

test("D1: the Home card shows while mixing, opens the page, and stops with a confirmation", async ({ page }) => {
  await open(page, "coinjoin-mixing");
  await expect(card(page)).toHaveAttribute("data-state", "mixing");
  await expect(page.getByTestId("coinjoin-card-percent")).toHaveText("42%");
  await expect(page.getByTestId("coinjoin-card-mixed")).toContainText(" of ");
  await page.getByTestId("coinjoin-card-action").click();
  await page.getByTestId("coinjoin-stop-confirm").click();
  // A stop the user asked for is not news: the card goes.
  await expect(card(page)).toHaveCount(0);
  await open(page, "coinjoin-paused");
  await page.getByTestId("coinjoin-card-open").click();
  await expect(page.getByTestId("page-coinJoin")).toBeVisible();
  await expect(status(page)).toHaveAttribute("data-state", "paused");
  await expect(page.getByTestId("coinjoin-status-line")).toHaveText("No Masternodes detected.");
});

test("discreet mode masks mixed of total", async ({ page }) => {
  await open(page, "coinjoin-mixing");
  await expect(page.getByTestId("coinjoin-card-mixed")).toContainText("tDASH");
  await page.getByTestId("hero-amount").click();
  await expect(page.getByTestId("coinjoin-card-mixed")).not.toContainText(/\d/);
});

test("a lock while mixing stops it; mixing-only unlock lets it start again (S10, D6)", async ({ page }) => {
  const errors = await open(page, "coinjoin-mixing");
  await expect(card(page)).toHaveAttribute("data-state", "mixing");
  await page.getByTestId("toolbar-lock").click();
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await page.getByTestId("lock-field").fill("demo");
  await page.getByTestId("lock-mixing-only").check();
  await page.getByTestId("lock-submit").click();
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await expect(card(page)).toHaveAttribute("data-state", "locked");
  await expect(page.getByTestId("coinjoin-card-line")).toHaveText("Mixing stopped because the wallet was locked.");
  // Dismiss hides this stop (D1).
  await page.getByTestId("coinjoin-card-dismiss").click();
  await expect(card(page)).toHaveCount(0);
  await navigate(page, "coinJoin");
  await expect(page.getByTestId("coinjoin-lock-line")).toHaveText("Unlocked for mixing only: mixing runs, sending stays locked.");
  await page.getByTestId("coinjoin-action").click();
  await expect(status(page)).toHaveAttribute("data-state", "starting");
  // Mixing again brings the card back.
  await navigate(page, "home");
  await expect(card(page)).toHaveAttribute("data-state", "starting");
  expect(errors).toEqual([]);
});

test("D9: a refused start says why and keeps the state", async ({ page }) => {
  await openPage(page, "funded");
  // CoinJoin turned off behind the page's back: the engine refuses.
  await page.evaluate(() => void (window.__dwFixture!.world.coinjoin.settings.enabled = false));
  await page.getByTestId("coinjoin-action").click();
  await expect(page.getByTestId("coinjoin-problem")).toHaveText("CoinJoin is turned off in Settings ▸ CoinJoin.");
  // The status is read again: Open Settings now.
  await expect(status(page)).toHaveAttribute("data-state", "disabled");
  await page.getByTestId("coinjoin-action").click();
  await expect(page.getByTestId("settings-coinjoin")).toBeVisible();
  await expect(page.getByTestId("settings-coinjoin-enabled")).not.toBeChecked();
});

test("S12: too little to mix, Start is disabled", async ({ page }) => {
  await openPage(page, "empty");
  await expect(page.getByTestId("coinjoin-action")).toBeDisabled();
  await expect(page.getByTestId("coinjoin-status-line")).toHaveText("CoinJoin requires at least 0.00140001 tDASH to use.");
});

test("D2: the level writes the network's rounds; another value is Custom", async ({ page }) => {
  await openPage(page, "funded");
  const radio = (id: string) => page.getByTestId(`coinjoin-level-${id}`).locator("input");
  await expect(radio("intermediate")).toBeChecked();
  await page.getByTestId("coinjoin-level-advanced").click();
  await expect(radio("advanced")).toBeChecked();
  expect(await page.evaluate(() => window.__dwFixture!.world.coinjoin.settings.rounds)).toBe(8);
  await expect(page.getByTestId("coinjoin-level-custom")).toHaveCount(0);
  await openPage(page, "coinjoin-custom");
  await expect(page.getByTestId("coinjoin-level-custom")).toContainText("Custom: 6 rounds");
  await expect(radio("intermediate")).not.toBeChecked();
  await expect(radio("advanced")).not.toBeChecked();
});

test("D11: Settings ▸ CoinJoin checks, saves and restores the network's options", async ({ page }) => {
  const errors = await openSettings(page, "coinjoin-mixing");
  const field = (f: string) => page.getByTestId(`settings-coinjoin-${f}`);
  const settings = () => page.evaluate(() => window.__dwFixture!.world.coinjoin.settings);
  await expect(field("save")).toBeDisabled();
  await expect(field("maxSessions")).toBeDisabled();
  await field("rounds").fill("20");
  await field("save").click();
  await expect(page.getByTestId("settings-coinjoin-problem")).toHaveText("Mixing rounds must be a whole number from 2 to 16.");
  await expect(field("rounds")).toHaveAttribute("aria-invalid", "true");
  expect((await settings()).rounds).toBe(4);
  await field("rounds").fill("6");
  await field("targetAmountDash").fill("2,500");
  await field("save").click();
  await expect(page.getByTestId("settings-coinjoin-done")).toHaveText("Saved.");
  expect(await settings()).toMatchObject({ rounds: 6, targetAmountDash: 2500 });
  await expect(field("rounds")).toHaveValue("6");
  // The level shows the custom value.
  await navigate(page, "coinJoin");
  await expect(page.getByTestId("coinjoin-level-custom")).toContainText("Custom: 6 rounds");
  // Turning CoinJoin off stops the mixing wallet.
  await navigate(page, "settings");
  await field("enabled").uncheck();
  await expect(field("enabled")).not.toBeChecked();
  expect(await page.evaluate((w) => window.__dwFixture!.world.coinjoin.mixers[w].stopReason, FIXTURE_WALLET_ID)).toBe("disabled");
  // Restore defaults, after a confirmation.
  await field("restore").click();
  await page.getByTestId("settings-coinjoin-restore-confirm").click();
  await expect(page.getByTestId("settings-coinjoin-done")).toHaveText("Saved.");
  expect(await settings()).toMatchObject({ enabled: true, rounds: 4, targetAmountDash: 1000 });
  await expect(field("enabled")).toBeChecked();
  expect(errors).toEqual([]);
});

test("D11: the engine's refusal shows its message", async ({ page }) => {
  await openSettings(page, "funded");
  await page.evaluate(() => window.__dwFixture!.fail("set_coinjoin_settings", "none"));
  await page.getByTestId("settings-coinjoin-multisession").click();
  await expect(page.getByTestId("settings-coinjoin-problem")).toBeVisible();
  await expect(page.getByTestId("settings-coinjoin-multisession")).not.toBeChecked();
});
