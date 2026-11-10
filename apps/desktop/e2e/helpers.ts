import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { expect, type Page, type TestInfo } from "@playwright/test";

export type Theme = "light" | "dark";
export const THEMES: Theme[] = ["light", "dark"];
export const SCREENS = ["overview", "transactions", "txDetail", "unlock"] as const;
export type Screen = (typeof SCREENS)[number];

/** Opens the fixture app's `funded` wallet and shows `screen`. */
export async function show(page: Page, screen: Screen, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  if (screen === "transactions" || screen === "txDetail") {
    await page.getByTestId("sidebar-activity").click();
    await page.getByTestId("tx-list").waitFor();
  }
  if (screen === "txDetail") {
    await page.locator("[data-txid]").first().click();
    await page.getByTestId("tx-detail-facts").waitFor();
  }
  if (screen === "unlock") {
    await page.getByTestId("toolbar-lock").click();
    await page.getByTestId("lock-screen").waitFor();
  }
  await fontsReady(page);
}

/** The fixture build's URL for a scenario (src/fixture/index.ts `fixtureOptionsFromUrl`). */
export function fixtureUrl(scenario: string, theme: Theme, extra = ""): string {
  return `/?scenario=${scenario}&theme=${theme}${extra}`;
}

/** Collects the page's uncaught errors; the returned list fills as they happen. */
export function trackPageErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  return errors;
}

/** Waits for web fonts, so text metrics and screenshots are final. */
export async function fontsReady(page: Page) {
  await page.evaluate(() => document.fonts.ready);
}

const repo = resolve(import.meta.dirname, "../../..");

/** Writes into docs/screenshots/gate when DW_WRITE_SCREENSHOTS=1, else into the test output. */
export function artifactPath(info: TestInfo, name: string): string {
  if (process.env.DW_WRITE_SCREENSHOTS === "1") {
    const p = resolve(repo, "docs/screenshots/gate", `playwright-${info.project.name}`, name);
    mkdirSync(dirname(p), { recursive: true });
    return p;
  }
  return info.outputPath(name);
}

export function writeJson(path: string, value: unknown) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value, null, 2));
}

/** The passphrase sheet of a pending grant, answered (the fixture's passphrase by default). */
export async function authorize(page: Page, passphrase = "demo") {
  const sheet = page.getByTestId("grant-sheet");
  await sheet.locator("input[type=password]").fill(passphrase);
  await sheet.locator("input[type=password]").press("Enter");
  await expect(sheet).toHaveCount(0);
}

/** Frames that come late, on demand: once `late(true)` runs, each `requestAnimationFrame`
 * callback waits `ms` first, as WebKit's frames do under load (BL-165). `settled()` switches it
 * off and waits until none is pending (a late frame may ask for another: that one is not late);
 * `window.__lateRan` counts the late callbacks run so far. Install before the page loads. */
export async function lateFrames(page: Page, ms = 500) {
  await page.addInitScript((ms) => {
    const w = window as unknown as { __late: boolean; __latePending: number; __lateRan: number };
    const raf = window.requestAnimationFrame.bind(window);
    w.__late = false;
    w.__latePending = 0;
    w.__lateRan = 0;
    window.requestAnimationFrame = (f) => {
      if (!w.__late) return raf(f);
      w.__latePending += 1;
      window.setTimeout(() => raf((t) => (w.__latePending -= 1, (w.__lateRan += 1), f(t))), ms);
      return 0;
    };
  }, ms);
  return {
    late: (on: boolean) => page.evaluate((on) => void ((window as unknown as { __late: boolean }).__late = on), on),
    settled: async () => {
      await page.evaluate(() => void ((window as unknown as { __late: boolean }).__late = false));
      await expect.poll(() => page.evaluate(() => (window as unknown as { __latePending: number }).__latePending)).toBe(0);
    },
  };
}

export type TransferRoute = "coreToShielded" | "coreToPlatform" | "shieldedToCore" | "platformToCore";

/** Home's Transfer sheet at `route`, opened the way Home offers it: the shortcut for the Dash wallet's
 * routes (picking Platform when it is not the default), a card's balance for its pool's. */
export async function openTransfer(page: Page, route: TransferRoute) {
  const body = page.getByTestId("transfer-body");
  if (route === "shieldedToCore") await page.getByTestId("shielded-card-transfer").click();
  else {
    await page.getByTestId("shortcut-transfer").click();
    await body.waitFor();
    if ((await body.getAttribute("data-view")) === "steps" && (await body.getAttribute("data-route")) !== route) await page.getByTestId("transfer-choose-route").click();
    if (route === "platformToCore") {
      if (!(await page.getByTestId("transfer-picker-platform").isVisible())) await page.getByTestId("transfer-from").click();
      await page.getByTestId("transfer-picker-platform").click();
      if ((await body.getAttribute("data-route")) !== route) {
        await page.getByTestId("transfer-to").click();
        await page.getByTestId("transfer-picker-core").click();
      }
    } else if ((await body.getAttribute("data-route")) !== route) {
      await page.getByTestId("transfer-to").click();
      await page.getByTestId(route === "coreToShielded" ? "transfer-picker-shielded" : "transfer-picker-platform").click();
    }
  }
  await expect(body).toHaveAttribute("data-route", route);
}

/** Platform → Dash wallet through Transfer, at the move's own confirmation. */
export async function moveFromPlatform(page: Page) {
  await openTransfer(page, "platformToCore");
  await page.getByTestId("transfer-continue").click();
  await expect(page.getByTestId("platform-move-body")).toBeVisible();
}

/** What Transfer from Platform says while it can't move (Continue waits); the sheet is closed after. */
export async function platformMoveWhy(page: Page): Promise<string | null> {
  await openTransfer(page, "platformToCore");
  await expect(page.getByTestId("transfer-continue")).toBeDisabled();
  const why = await page.getByTestId("transfer-unavailable").textContent();
  await page.getByTestId("transfer-cancel").click();
  await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
  return why;
}
