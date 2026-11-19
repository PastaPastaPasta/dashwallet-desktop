import { expect, test, type Locator, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// Receive's Dash / Platform / Shielded tabs switch the address only: the request column keeps its
// place and size on every tab, and on Platform and Shielded it says requests are for Dash addresses.

const TAB = { dash: "receive-select-dash", platform: "platform-receive-toggle", shielded: "shielded-receive-open" } as const;
type Tab = keyof typeof TAB;
type Box = { x: number; y: number; width: number; height: number };

async function open(page: Page, theme: (typeof THEMES)[number]) {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("hero").waitFor();
  await page.getByTestId("sidebar-receive").click();
  await expect(page.getByTestId("receive-qr")).toBeVisible();
  return errors;
}

/** Picks `tab` and waits for its address; the boxes of the request column, the card it shows and the address column. */
async function pick(page: Page, tab: Tab) {
  await page.getByTestId(TAB[tab]).click();
  await expect(page.getByTestId(TAB[tab])).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId(tab === "dash" ? "receive-qr" : `${tab}-receive-qr`)).toBeVisible();
  const column = page.getByTestId("receive-request-column");
  const card = page.getByTestId(tab === "dash" ? "request-form" : "receive-request-unavailable");
  await expect(column).toBeVisible();
  await expect(card).toBeVisible();
  const box = async (l: ReturnType<Page["locator"]>) => (await l.boundingBox()) as Box;
  return { column: await box(column), card: await box(card), left: await box(page.locator(".receive-columns > section").first()), tabs: await box(page.getByRole("group", { name: "Receive in" })) };
}

type Rgba = [number, number, number, number];
const rgba = (c: string): Rgba => {
  const [r, g, b, a = 1] = c.match(/[\d.]+/g)!.map(Number);
  return [r, g, b, a];
};
const over = (top: Rgba, under: Rgba): Rgba => [0, 1, 2].map((i) => top[i] * top[3] + under[i] * (1 - top[3])).concat(1) as Rgba;
const luminance = ([r, g, b]: Rgba) =>
  [r, g, b].map((v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)).reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);

/** The tick's contrast with the line's background, laid over the nearest opaque ancestor. */
async function tickContrast(line: Locator): Promise<number> {
  const { tick, layers } = await line.evaluate((p) => {
    const layers: string[] = [];
    for (let e: Element | null = p; e; e = e.parentElement) {
      const bg = getComputedStyle(e).backgroundColor;
      layers.push(bg);
      if (!/rgba|\/ /.test(bg) || bg.endsWith(", 1)")) break;
    }
    return { tick: getComputedStyle(p.querySelector("span")!).color, layers };
  });
  const back = layers.map(rgba).reduceRight((under, top) => over(top, under), [255, 255, 255, 1] as Rgba);
  const [a, b] = [luminance(over(rgba(tick), back)), luminance(back)];
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function same(a: Box, b: Box, what: string) {
  for (const k of ["x", "y", "width", "height"] as const) expect(Math.abs(a[k] - b[k]), `${what} ${k}: ${a[k]} vs ${b[k]}`).toBeLessThanOrEqual(1);
}

for (const theme of THEMES) {
  test(`the request column keeps its place and size on every tab, both ways (${theme})`, async ({ page }) => {
    const errors = await open(page, theme);
    const first = await pick(page, "dash");
    // The tabs are above both columns.
    expect(first.tabs.y + first.tabs.height).toBeLessThanOrEqual(Math.min(first.left.y, first.column.y));
    for (const tab of ["platform", "shielded", "dash", "shielded", "platform", "dash"] as const) {
      const now = await pick(page, tab);
      same(now.column, first.column, `${tab} request column`);
      same(now.card, first.column, `${tab} request card`);
      expect(Math.abs(now.left.y - first.left.y), `${tab} address column top`).toBeLessThanOrEqual(1);
      same(now.tabs, first.tabs, `${tab} tabs`);
    }
    expect(errors).toEqual([]);
  });
}

for (const theme of THEMES) {
  test(`#67 a payment that arrived says so with a tick and text that meet AA contrast (${theme})`, async ({ page }, info) => {
    const errors = await open(page, theme);
    const address = (await page.getByTestId("receive-address").textContent())!;
    await page.evaluate(`window.__dwFixture.receivePayment(150000000, undefined, ${JSON.stringify(address)})`);
    const received = page.getByTestId("receive-received");
    await expect(received).toHaveText("✓ Received 1.50 tDASH");
    // The tick is aria-hidden, so axe skips it; as a graphic it needs 3:1 against the tint it sits on.
    // Measured first: the line shows for RECEIVED_HOLD_MS only.
    expect(await tickContrast(received)).toBeGreaterThanOrEqual(3);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await fontsReady(page);
    await page.screenshot({ path: artifactPath(info, `${theme}-receive-received.png`) });
    expect(errors).toEqual([]);
  });
}

for (const pool of ["platform", "shielded"] as const) {
  test(`${pool}: the request column says what the address is for, and that requests are for Dash addresses`, async ({ page }) => {
    await open(page, "light");
    await page.getByTestId("request-label").fill("Draft rent");
    await pick(page, pool);
    const panel = page.getByTestId("receive-request-unavailable");
    await expect(panel).toHaveAttribute("data-pool", pool);
    await expect(panel.getByRole("heading")).toHaveText("About this address");
    await expect(panel).toContainText(pool === "platform" ? "This address holds Dash on Dash Platform. Anyone with a Platform address can pay it." : "Money sent to this address stays private. Anyone with a Shielded balance can pay it.");
    await expect(panel).toContainText("Payment requests are for Dash addresses only.");
    if (pool === "platform") await expect(panel.getByTestId("platform-receive-warning")).toHaveText("Most Dash wallets and exchanges cannot pay this address.");
    else await expect(page.getByTestId("platform-receive-warning")).toHaveCount(0);
    // No request form: not shown, not reachable.
    for (const id of ["request-form", "request-label", "request-create"]) await expect(page.getByTestId(id)).toBeHidden();
    expect(await page.getByTestId("request-form").evaluate((f) => (f as HTMLFormElement).inert)).toBe(true);
    expect(await seriousAxeViolations(page)).toEqual([]);
    // Back on Dash the form is as it was left.
    await pick(page, "dash");
    await expect(panel).toHaveCount(0);
    await expect(page.getByTestId("request-label")).toHaveValue("Draft rent");
    await expect(page.getByTestId("request-create")).toBeEnabled();
  });
}
