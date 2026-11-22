import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { FIXTURE_PASSPHRASE, type World } from "../src/fixture/scenarios";
import { fixtureUrl, fontsReady, THEMES } from "./helpers";
import { PAYEE } from "./wallet";

const OUT = process.env.DW_SHOTS_OUT!;
const REV = process.env.DW_SHOTS_REV!;
test.describe.configure({ timeout: 90_000 });

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;

async function tabTo(page: Page, testId: string) {
  const t = page.getByTestId(testId);
  await t.waitFor();
  for (let i = 0; i < 60; i++) {
    if (await t.evaluate((el) => el === document.activeElement)) return;
    await page.keyboard.press("Tab");
  }
  throw new Error(`never reached ${testId}`);
}
async function unlock(page: Page) {
  await tabTo(page, "lock-field");
  await page.keyboard.type(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}
async function review(page: Page) {
  await page.evaluate((address) => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address, amount: 50_000_000, label: null, message: null }, username: null }]), PAYEE);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.50000000");
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await tabTo(page, "pay-continue");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("pay-confirm")).toBeVisible();
}
async function sendHeld(page: Page) {
  await review(page);
  await expect(page.getByTestId("pay-confirm-passphrase")).toBeFocused();
  await page.keyboard.type(FIXTURE_PASSPHRASE);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("draft_broadcast"))).toBe(true);
  return () => release.evaluate((r) => r());
}
const settle = (page: Page) => page.evaluate(() => new Promise((r) => setTimeout(() => requestAnimationFrame(() => requestAnimationFrame(r)), 400)));

async function shoot(page: Page, name: string, theme: string) {
  await settle(page);
  mkdirSync(OUT, { recursive: true });
  await page.screenshot({ path: `${OUT}/${name}-${theme}-${REV}.png` });
  await page.evaluate(() => {
    const a = document.activeElement as HTMLElement | null;
    const body = !a || a === document.body;
    const r = body ? { left: 4, top: 4, width: innerWidth - 8, height: innerHeight - 8 } : a.getBoundingClientRect();
    const id = body ? "<body> (outside the sheet)" : a.getAttribute("data-testid") ?? (a.getAttribute("role") === "dialog" ? `dialog panel (${a.getAttribute("data-testid")})` : a.tagName.toLowerCase());
    const box = document.createElement("div");
    Object.assign(box.style, { position: "fixed", left: `${r.left - 3}px`, top: `${r.top - 3}px`, width: `${r.width + 6}px`, height: `${r.height + 6}px`, outline: "3px dashed #ff00ff", zIndex: "2147483647", pointerEvents: "none", boxSizing: "border-box" });
    const label = document.createElement("div");
    label.textContent = `annotation · document.activeElement: ${id}`;
    Object.assign(label.style, { position: "fixed", left: `${Math.max(4, r.left)}px`, top: `${body ? 10 : Math.max(4, r.top - 26)}px`, background: "#ff00ff", color: "#fff", font: "600 13px/20px system-ui, sans-serif", padding: "0 6px", zIndex: "2147483647", pointerEvents: "none" });
    document.documentElement.append(box, label);
  });
  await page.screenshot({ path: `${OUT}/${name}-${theme}-${REV}-focus.png` });
}

for (const theme of THEMES) {
  test(`capture 1 sending sheet shift-tab ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("locked", theme));
    await page.getByTestId("lock-field").waitFor();
    await fontsReady(page);
    await unlock(page);
    await sendHeld(page);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
    await unlock(page);
    await expect(page.getByTestId("pay-sending")).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await shoot(page, "1-sending-shift-tab", theme);
  });

  test(`capture 2 create unsigned failed ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("watch-only", theme));
    await page.getByTestId("toolbar").waitFor();
    await fontsReady(page);
    await review(page);
    await tabTo(page, "pay-confirm-unsigned");
    const release = await page.evaluateHandle(() => {
      window.__dwFixture!.fail("draft_create_unsigned", "rpc");
      return window.__dwFixture!.hold("draft_create_unsigned");
    });
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("pay-confirm-unsigned")).toBeDisabled();
    await settle(page);
    await release.evaluate((r) => r());
    await expect(page.getByTestId("pay-confirm-error")).not.toBeEmpty();
    await expect(page.getByTestId("pay-confirm-unsigned")).toBeEnabled();
    await shoot(page, "2-create-unsigned-failed", theme);
  });

  test(`capture 3 try again running ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await page.getByTestId("toolbar").waitFor();
    await fontsReady(page);
    await world(page, (w) => (w.send.broadcast = "unknown"));
    await (await sendHeld(page))();
    await expect(page.getByTestId("pay-result-retry")).toBeFocused();
    await world(page, (w) => (w.send.broadcast = "ok"));
    await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("pay-result-retry")).toBeDisabled();
    await shoot(page, "3-try-again-running", theme);
  });

  test(`capture 4 tx detail tab past disclosure ${theme}`, async ({ page }) => {
    await page.goto(fixtureUrl("funded", theme));
    await page.getByTestId("toolbar").waitFor();
    await fontsReady(page);
    await tabTo(page, "sidebar-activity");
    await page.keyboard.press("Enter");
    const row = page.locator("[data-txid]").first();
    await row.waitFor();
    for (let i = 0; i < 60 && !(await row.evaluate((el) => el === document.activeElement)); i++) await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    const summary = page.getByTestId("tx-detail-technical").locator("summary");
    await summary.waitFor();
    for (let i = 0; i < 60 && !(await summary.evaluate((el) => el === document.activeElement)); i++) await page.keyboard.press("Tab");
    await expect(summary).toBeFocused();
    await page.keyboard.press("Tab");
    await shoot(page, "4-tx-detail-tab-past-disclosure", theme);
  });
}
