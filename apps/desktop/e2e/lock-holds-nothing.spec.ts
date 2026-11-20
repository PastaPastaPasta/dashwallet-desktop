import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE, type World } from "../src/fixture/scenarios";
import { fixtureUrl, trackPageErrors } from "./helpers";

// A locked window holds nothing of the wallet: the lock screen stands in place of the pages, not
// over them, so the page's text and markup carry no balance, wallet name, history row, contact or
// address until the unlock, and the renderer reads none of them meanwhile.

const BALANCE = "32.04698287";

/** What the renderer may ask the engine while locked: the window, the lock and its report, the menus. */
const LOCKED_CALLS = new Set(["launch_info", "ui_settings", "open_network", "lock_status", "last_lock_report", "shell_sync", "take_links"]);

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;
const calls = (page: Page) => page.evaluate(() => [...window.__dwFixture!.calls]);

/** The wallet's own words the fixture knows: its name, its contacts' names, its addresses and labels. */
const walletWords = (page: Page) =>
  world(page, (w) => {
    const wallet = w.wallets[0];
    const words = [wallet.summary.name];
    for (const c of w.dashpay.contacts) words.push(c.summary.username ?? "", c.profile?.displayName ?? "");
    for (const r of wallet.rows) words.push(r.address ?? "", r.label ?? "");
    return words.filter((s) => s.length >= 3);
  });

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The whole document as text and markup: what an assistive tool or a capture could read. */
const pageHolds = (page: Page) => page.evaluate(() => `${document.body.innerText}\n${document.documentElement.outerHTML}\n${document.title}`);

async function expectHoldsNothing(page: Page, words: string[], rows: string[]) {
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await expect(page.locator(".app-chrome")).toHaveCount(0);
  const held = await pageHolds(page);
  expect(held).not.toContain(BALANCE);
  for (const w of words) expect(held, w).not.toMatch(new RegExp(`\\b${escape(w)}\\b`));
  for (const r of rows) expect(held, r).not.toContain(r);
}

async function unlock(page: Page) {
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("lock-field").press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}

/** The text of the first few history rows on screen. */
const rowTexts = (page: Page) => page.locator("[data-txid]").evaluateAll((els) => els.slice(0, 3).map((e) => (e as HTMLElement).innerText.split("\n").find((l) => /\d/.test(l)) ?? ""));

test("a launch while locked holds and reads nothing of the wallet until the unlock", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  expect(words.length).toBeGreaterThan(3);
  await expectHoldsNothing(page, words, []);
  const before = await page.evaluate(() => document.documentElement.outerHTML);
  const asked = await calls(page);
  expect(asked.filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);

  await unlock(page);
  await expect(page.getByTestId("hero")).toContainText(BALANCE);
  await expect(page.locator(".toolbar-wallet")).toHaveText("Main");
  await expect(page.locator("[data-txid]").first()).toBeVisible();
  const rows = (await rowTexts(page)).filter(Boolean);
  expect(rows.length).toBeGreaterThan(0);
  for (const r of rows) expect(before).not.toContain(r);
  expect(errors).toEqual([]);
});

test("a lock while in use takes the pages down, and the unlock lands on the page it was on", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  await unlock(page);
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  const rows = (await rowTexts(page)).filter(Boolean);
  expect(rows.length).toBeGreaterThan(0);

  await page.getByTestId("toolbar-lock").click();
  const asked = (await calls(page)).length;
  await expectHoldsNothing(page, words, rows);
  // Nothing mounted reads the wallet while locked, whatever the engine signals.
  await page.evaluate(() => window.__dwFixture!.emit({ kind: "balances", network: "testnet", walletId: window.__dwFixture!.world.wallets[0].summary.walletId, balances: null }));
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  expect((await calls(page)).slice(asked).filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);
  await expectHoldsNothing(page, words, rows);

  await unlock(page);
  await expect(page.getByTestId("tx-list")).toBeVisible();
  await expect(page.getByTestId("sidebar-activity")).toHaveAttribute("aria-current", "page");
  await expect(page.locator("[data-txid]").first()).toBeVisible();
  expect((await rowTexts(page)).filter(Boolean)).toEqual(rows);
  expect(errors).toEqual([]);
});
