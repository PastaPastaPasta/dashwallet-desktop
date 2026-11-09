// Row 74 DEC-1484 capture (scratch, not committed): the network choice's states, light and dark.
// REV=base|head; head-only states are skipped at base. Shots go to $DW_SHOTS/<rev>/<theme>-<state>.png.
import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, type Theme } from "./helpers";

const rev = process.env.REV as "base" | "head";
const out = `${process.env.DW_SHOTS}/${rev}`;
const THEMES: Theme[] = ["light", "dark"];

async function shot(page: Page, theme: Theme, name: string) {
  await fontsReady(page);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: `${out}/${theme}-${name}.png` });
}
const go = (page: Page, scenario: string, theme: Theme, extra = "") => page.goto(fixtureUrl(scenario, theme, extra));
const toSettings = async (page: Page) => {
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-network-choice").waitFor();
};
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const knobs = (page: Page, k: any) => page.evaluate((k) => void ((window.__dwFixture!.world as any).networkSwitch = k), k);
const hold = (page: Page, cmd: string) => page.evaluateHandle((c) => window.__dwFixture!.hold(c as never), cmd);
const headOnly = () => test.skip(rev !== "head", "a state the base does not have");

for (const theme of THEMES) {
  test.describe(theme, () => {
    test("welcome-testnet-chosen", async ({ page }) => {
      await go(page, "no-wallet-mainnet", theme);
      await expect(page.getByTestId("onboarding-network-mainnet")).toHaveAttribute("aria-pressed", "true");
      await expect(page.getByTestId("onboarding-create")).toBeEnabled();
      await page.getByTestId("onboarding-network-testnet").click();
      if (rev === "base") {
        await expect(page.getByTestId("onboarding-network-restart")).toContainText("Restart Dash Wallet to use Testnet.");
        await expect(page.getByTestId("onboarding-create")).toBeDisabled();
      } else {
        await expect(page.getByTestId("onboarding-network-testnet")).toHaveAttribute("aria-pressed", "true");
        await expect(page.getByTestId("network-switching")).toHaveCount(0);
        await expect(page.getByTestId("onboarding-create")).toBeEnabled();
      }
      await shot(page, theme, "welcome-testnet-chosen");
    });

    test("settings-picker", async ({ page }) => {
      await go(page, "funded", theme);
      await toSettings(page);
      await expect(page.getByTestId("settings-network-choice-testnet")).toHaveAttribute("aria-pressed", "true");
      await page.getByTestId("settings-network-choice").evaluate((e) => e.scrollIntoView({ block: "center" }));
      await shot(page, theme, "settings-picker");
    });

    test("welcome-switching", async ({ page }) => {
      headOnly();
      await go(page, "no-wallet-mainnet", theme);
      await expect(page.getByTestId("onboarding-create")).toBeEnabled();
      await hold(page, "switch_network");
      await page.getByTestId("onboarding-network-testnet").click();
      await expect(page.getByTestId("network-switching")).toContainText("Opening Testnet…");
      await shot(page, theme, "welcome-switching");
    });

    test("settings-mainnet-chosen", async ({ page }) => {
      await go(page, "funded", theme);
      await toSettings(page);
      await page.getByTestId("settings-network-choice-mainnet").click();
      if (rev === "base") {
        await expect(page.getByTestId("settings-network-choice-restart")).toContainText("Restart Dash Wallet to use Mainnet.");
      } else {
        await expect(page.getByTestId("network-switch-sheet")).toContainText("Switch to Mainnet?");
        await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
      }
      await shot(page, theme, "settings-mainnet-chosen");
    });

    test("settings-confirm-pending", async ({ page }) => {
      headOnly();
      await go(page, "recovery-one", theme);
      await toSettings(page);
      await page.getByTestId("settings-network-choice-mainnet").click();
      await expect(page.getByTestId("network-switch-pending")).toBeVisible();
      await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
      await shot(page, theme, "settings-confirm-pending");
    });

    test("settings-confirm-checking", async ({ page }) => {
      headOnly();
      await go(page, "recovery-one", theme, "&hold=list_wallets");
      await toSettings(page);
      await page.getByTestId("settings-network-choice-mainnet").click();
      await expect(page.getByTestId("network-switch-checking")).toHaveText("Checking for unfinished transfers…");
      await expect(page.getByTestId("network-switch-confirm")).toBeDisabled();
      await shot(page, theme, "settings-confirm-checking");
    });

    test("settings-switching", async ({ page }) => {
      headOnly();
      await go(page, "funded", theme);
      await toSettings(page);
      await page.getByTestId("settings-network-choice-mainnet").click();
      await expect(page.getByTestId("network-switch-confirm")).toBeEnabled();
      await hold(page, "switch_network");
      await page.getByTestId("network-switch-confirm").click();
      await expect(page.getByTestId("network-switching")).toContainText("Opening Mainnet…");
      await shot(page, theme, "settings-switching");
    });

    const refusals: [string, unknown, string][] = [
      ["settings-busy", { busy: { flows: ["registration", "topUp"], mixing: true, broadcast: true } }, "Wait for the payment, username registration and credit top-up to finish, or stop mixing, before switching networks."],
      ["settings-busy-lock", { busy: { flows: ["contactRequest"], mixing: false, broadcast: false, stalled: true } }, "Locking the wallet stops contact request."],
      ["settings-busy-draining", { busy: { flows: [], mixing: false, broadcast: false, draining: true } }, "Dash Wallet is still finishing a request on this network. Try switching again once it is done."],
      ["settings-close-refused-wallet", { closeRefused: "wallet" }, "A wallet is still opening. Switch networks once it is open."],
      ["settings-close-refused-shielded", { closeRefused: "shielded_spend_unresolved" }, "A shielded payment is still being confirmed. Switch networks once it is."],
    ];
    for (const [name, k, text] of refusals) {
      test(name, async ({ page }) => {
        headOnly();
        await go(page, "funded", theme);
        await toSettings(page);
        await knobs(page, k);
        await page.getByTestId("settings-network-choice-mainnet").click();
        await page.getByTestId("network-switch-confirm").click();
        await expect(page.getByTestId("settings-network-choice-problem")).toContainText(text);
        await shot(page, theme, name);
      });
    }

    test("switch-rolled-back", async ({ page }) => {
      headOnly();
      await go(page, "funded", theme);
      await toSettings(page);
      await knobs(page, { fail: { rolledBack: true } });
      await page.getByTestId("settings-network-choice-mainnet").click();
      await page.getByTestId("network-switch-confirm").click();
      await expect(page.locator(".toast")).toContainText("Mainnet did not open. Dash Wallet is back on Testnet.");
      await expect(page.getByTestId("lock-screen")).toBeVisible();
      await shot(page, theme, "switch-rolled-back");
    });

    test("switch-none-open", async ({ page }) => {
      headOnly();
      await go(page, "funded", theme);
      await toSettings(page);
      await knobs(page, { fail: { rolledBack: false } });
      await page.getByTestId("settings-network-choice-mainnet").click();
      await page.getByTestId("network-switch-confirm").click();
      await expect(page.locator(".toast")).toContainText("Mainnet did not open, and neither did Testnet again.");
      await expect(page.getByTestId("splash").getByRole("alert")).toBeVisible();
      await shot(page, theme, "switch-none-open");
    });

    test("switch-save-failed", async ({ page }) => {
      headOnly();
      await go(page, "no-wallet", theme);
      await expect(page.getByTestId("onboarding-create")).toBeEnabled();
      await knobs(page, { saveFails: true });
      await page.getByTestId("onboarding-network-mainnet").click();
      await expect(page.locator(".toast")).toContainText("The network choice could not be saved.");
      await expect(page.getByTestId("onboarding-network-mainnet")).toHaveAttribute("aria-pressed", "true");
      await shot(page, theme, "switch-save-failed");
    });

    test("settings-pinned", async ({ page }) => {
      headOnly();
      await go(page, "network-pinned", theme);
      await toSettings(page);
      await expect(page.getByTestId("settings-network-choice-pinned")).toBeVisible();
      await page.getByTestId("settings-network-choice-pinned").scrollIntoViewIfNeeded();
      await shot(page, theme, "settings-pinned");
    });

    test("add-wallet", async ({ page }) => {
      await go(page, "funded", theme);
      await page.getByTestId("toolbar").waitFor();
      await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "addWallet" }));
      await expect(page.getByText("Add a wallet")).toBeVisible();
      await expect(page.getByTestId("onboarding-create")).toBeVisible();
      await shot(page, theme, "add-wallet");
    });
  });
}
