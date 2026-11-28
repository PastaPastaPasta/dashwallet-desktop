import { expect, test, type Page } from "@playwright/test";
import type { Route } from "../src/api/generated/Route";
import { fixtureUrl, fontsReady, THEMES, type Theme } from "./helpers";

// Every page, and the panes and sheets that open inside one, fits the window from 1024 px up in
// both appearances: nothing scrolls sideways, and nothing pokes out of the main pane. Checkboxes
// and radios are drawn from tokens, so an unchecked one reads as empty on every engine.

/** A colour token as the page resolves it. */
const tokenColor = (page: Page, token: string) =>
  page.evaluate((t) => {
    const probe = document.createElement("span");
    probe.style.color = `var(${t})`;
    document.body.append(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  }, token);

/** The mark a checked control draws: its `::before` content, the tick's stroke and the dot's fill. */
const mark = (el: Element) => {
  const s = getComputedStyle(el, "::before");
  return { content: s.content, stroke: s.borderLeftColor, strokeWidth: s.borderLeftWidth, fill: s.backgroundColor };
};

const ROUTES: Route[] = [
  "home",
  "pay",
  "receive",
  "contacts",
  "activity",
  "coinJoin",
  "explore",
  "masternodes",
  "notifications",
  "profile",
  "settings",
  "tools",
  "joinDashPay",
];

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 820 },
];

/** What scrolls sideways or sticks out of the main pane, empty when the page fits. */
function overflow(page: Page) {
  return page.evaluate(() => {
    const found: string[] = [];
    const doc = document.scrollingElement!;
    if (doc.scrollWidth > doc.clientWidth) found.push(`document ${doc.scrollWidth} > ${doc.clientWidth}`);
    const name = (el: Element) => `${el.tagName.toLowerCase()}${el.getAttribute("data-testid") ? `[${el.getAttribute("data-testid")}]` : ""}.${[...el.classList].join(".")}`;
    for (const el of document.querySelectorAll<HTMLElement>("body *")) {
      const x = getComputedStyle(el).overflowX;
      if ((x === "auto" || x === "scroll") && el.scrollWidth > el.clientWidth + 1) found.push(`${name(el)} scrolls ${el.scrollWidth} > ${el.clientWidth}`);
    }
    const main = document.querySelector("main.page");
    if (main) {
      const right = main.getBoundingClientRect().right;
      for (const el of main.querySelectorAll("*")) {
        const box = el.getBoundingClientRect();
        if (box.width > 0 && box.right > right + 1) found.push(`${name(el)} ends at ${Math.round(box.right)} > ${Math.round(right)}`);
      }
    }
    return found;
  });
}

async function open(page: Page, route: Route, theme: Theme) {
  await page.goto(fixtureUrl(route === "masternodes" ? "governance" : "dashpay-registered", theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);
  await expect(page.getByTestId(`page-${route}`)).toBeVisible();
  await fontsReady(page);
}

/** The panes and sheets a page opens in place, each from its page's fresh state. */
const PANES: { name: string; route: Route; open: (page: Page) => Promise<void> }[] = [
  {
    name: "Contacts with Add contact open",
    route: "contacts",
    open: async (page) => {
      await page.getByTestId("contacts-add").click();
      await expect(page.getByTestId("add-contact")).toBeVisible();
    },
  },
  {
    name: "Pay to an address with Advanced open",
    route: "pay",
    open: async (page) => {
      await page.getByTestId("pay-advanced").locator("summary").click();
      await expect(page.getByTestId("pay-advanced")).toHaveAttribute("open", "");
    },
  },
  {
    name: "Activity with a transaction open",
    route: "activity",
    open: async (page) => {
      await page.locator("[data-txid]").first().click();
      await expect(page.getByTestId("tx-detail-facts")).toBeVisible();
    },
  },
];

for (const theme of THEMES) {
  for (const route of ROUTES) {
    test(`${route} fits 1024 and 1280 without sideways scrolling, ${theme}`, async ({ page }) => {
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await open(page, route, theme);
        expect(await overflow(page), `${route} at ${size.width}`).toEqual([]);
      }
    });
  }

  for (const pane of PANES) {
    test(`${pane.name} fits 1024 and 1280 without sideways scrolling, ${theme}`, async ({ page }) => {
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await open(page, pane.route, theme);
        await pane.open(page);
        expect(await overflow(page), `${pane.name} at ${size.width}`).toEqual([]);
      }
    });
  }

  test(`Contacts keeps Add contact on one line while its pane is open at 1024, ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await open(page, "contacts", theme);
    const add = page.getByTestId("contacts-add");
    const closed = (await add.boundingBox())!;
    await add.click();
    await expect(page.getByTestId("add-contact")).toBeVisible();
    await expect(add).toBeDisabled();
    await expect(add).toHaveAttribute("aria-expanded", "true");
    // The open pane's own title says why the button waits; the header adds no note.
    await expect(add).toHaveAccessibleDescription("Add a New Contact");
    const header = page.getByTestId("page-contacts").locator(".top-intro-row");
    await expect(header.locator(".dash-button-why")).toHaveCount(0);
    expect((await add.boundingBox())!.height).toBe(closed.height);
    const label = await add.evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      return new Set([...range.getClientRects()].map((r) => Math.round(r.top))).size;
    });
    expect(label, "Add contact's label lines").toBe(1);
    expect(await header.evaluate((el) => el.scrollWidth - el.clientWidth)).toBe(0);
  });

  test(`an unchecked checkbox is an empty outline, a checked one fills, ${theme}`, async ({ page }) => {
    await open(page, "pay", theme);
    const box = page.getByTestId("pay-subtract-fee");
    await expect(box).not.toBeChecked();
    const look = () =>
      box.evaluate((el) => {
        const s = getComputedStyle(el);
        const probe = document.createElement("span");
        probe.style.color = "var(--dw-role-accent)";
        document.body.append(probe);
        const accent = getComputedStyle(probe).color;
        probe.remove();
        return { appearance: s.appearance, background: s.backgroundColor, border: s.borderTopColor, borderWidth: s.borderTopWidth, accent };
      });
    const unchecked = await look();
    // Drawn by the stylesheet, not the engine: WebKitGTK fills a native unchecked box white in dark.
    expect(unchecked.appearance).toBe("none");
    expect(unchecked.background).toBe("rgba(0, 0, 0, 0)");
    expect(unchecked.border).not.toBe("rgba(0, 0, 0, 0)");
    expect(parseFloat(unchecked.borderWidth)).toBeGreaterThan(0);
    expect((await box.evaluate(mark)).content).toBe("none");
    await box.check();
    const checked = await look();
    expect(checked.background).toBe(checked.accent);
    // The tick is drawn in the text-on-accent colour.
    const tick = await box.evaluate(mark);
    expect(tick.content).toBe('""');
    expect(tick.stroke).toBe(await tokenColor(page, "--dw-role-text-on-hero"));
    expect(parseFloat(tick.strokeWidth)).toBeGreaterThan(0);
  });

  test(`a checked radio fills and draws its dot, an unchecked one stays an outline, ${theme}`, async ({ page }) => {
    await open(page, "pay", theme);
    await page.locator("summary", { hasText: "Advanced" }).first().click();
    const radios = page.locator('input[type="radio"]');
    await expect(radios.first()).toBeVisible();
    const looks = await radios.evaluateAll((els) =>
      els.map((el) => {
        const s = getComputedStyle(el);
        const dot = getComputedStyle(el, "::before");
        return { checked: (el as HTMLInputElement).checked, fill: s.backgroundColor, radius: s.borderTopLeftRadius, content: dot.content, dot: dot.backgroundColor };
      }),
    );
    const accent = await tokenColor(page, "--dw-role-accent");
    const white = await tokenColor(page, "--dw-role-text-on-hero");
    const on = looks.filter((l) => l.checked);
    const off = looks.filter((l) => !l.checked);
    expect(on.length).toBeGreaterThan(0);
    expect(off.length).toBeGreaterThan(0);
    for (const l of on) expect(l).toMatchObject({ fill: accent, radius: "50%", content: '""', dot: white });
    for (const l of off) expect(l).toMatchObject({ fill: "rgba(0, 0, 0, 0)", radius: "50%", content: "none" });
  });

  test(`every checkbox and radio on Settings is drawn from tokens, ${theme}`, async ({ page }) => {
    await open(page, "settings", theme);
    const inputs = page.locator('input[type="checkbox"], input[type="radio"]');
    const looks = await inputs.evaluateAll((els) =>
      els.map((el) => {
        const s = getComputedStyle(el);
        const filled = s.backgroundColor !== "rgba(0, 0, 0, 0)";
        return `${s.appearance} ${(el as HTMLInputElement).checked === filled ? "fill follows checked" : "fill wrong"} ${s.borderTopColor === "rgba(0, 0, 0, 0)" ? "no outline" : "outlined"}`;
      }),
    );
    expect(looks.length).toBeGreaterThan(0);
    expect(new Set(looks)).toEqual(new Set(["none fill follows checked outlined"]));
  });

  test(`the lock screen's checkbox is white on hero blue, its tick blue, ${theme}`, async ({ page }) => {
    await open(page, "home", theme);
    await page.getByTestId("toolbar-lock").click();
    const box = page.getByTestId("lock-mixing-only");
    const look = () =>
      box.evaluate((el) => {
        const probe = document.createElement("span");
        probe.style.color = "var(--dw-role-text-on-hero)";
        document.body.append(probe);
        const white = getComputedStyle(probe).color;
        probe.remove();
        const s = getComputedStyle(el);
        return { white, background: s.backgroundColor, border: s.borderTopColor, appearance: s.appearance };
      });
    const unchecked = await look();
    expect(unchecked).toEqual({ white: unchecked.white, background: "rgba(0, 0, 0, 0)", border: unchecked.white, appearance: "none" });
    await box.check();
    const checked = await look();
    expect(checked.background).toBe(checked.white);
    // The tick is hero blue on the white box.
    const tick = await box.evaluate(mark);
    expect(tick.content).toBe('""');
    expect(tick.stroke).toBe(await tokenColor(page, "--dw-role-hero"));
  });
}
