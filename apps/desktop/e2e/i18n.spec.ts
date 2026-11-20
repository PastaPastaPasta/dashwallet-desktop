import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// Localization (MP-14): the language picker in Settings ▸ General, and the converted area (the shell,
// the lock screen and Settings ▸ General) under the two pseudo-locales: `en-XA` brackets, accents and
// lengthens every catalog string, so text that bypasses the catalog stays plain ASCII and a cut-off
// string loses its bracket; `ar-XB` turns the window right to left.

const SIZES = [
  { width: 1280, height: 820 },
  // The window's minimum (dw-app tauri.conf.json).
  { width: 920, height: 600 },
];

/** Text that is data, not copy: the fixture's username, its avatar initial and wallet name, units and paths. */
const DATA = [/^@[\w.-]+$/, /^\p{Lu}$/u, /^Main$/, /^(t?DASH|mDASH|μDASH|duffs)$/, /^\/[\w./-]+$/];

/** The visible text in `areas` that bypasses the catalog: plain Latin text that is no catalog
 * string's and is not data. */
async function untranslated(page: Page, areas: string[], kind: "long" | "rtl"): Promise<string[]> {
  const found = await page.evaluate(
    ({ areas, kind }) => {
      const out: string[] = [];
      for (const testId of areas) {
        const root = document.querySelector(`[data-testid="${testId}"]`);
        if (!root) continue;
        const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = walk.nextNode(); n; n = walk.nextNode()) {
          const el = n.parentElement!;
          const text = n.textContent!.trim();
          if (!text || !/[A-Za-z]/.test(text)) continue;
          if (el.closest("option") && !(el.closest("option") as HTMLOptionElement).selected) continue;
          if (!el.checkVisibility({ visibilityProperty: true, opacityProperty: false })) continue;
          const catalog = kind === "long" ? /[⟦⟧]/.test(text) : text.includes("‮");
          if (!catalog) out.push(text);
        }
      }
      return out;
    },
    { areas, kind },
  );
  return found.filter((text) => !DATA.some((re) => re.test(text)));
}

/** What scrolls sideways or sticks out of the window or its pane. */
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
    for (const area of document.querySelectorAll('[data-testid="toolbar"], [data-testid="sidebar"], [data-testid="status-bar"], main.page, [data-testid="lock-column"]')) {
      const box = area.getBoundingClientRect();
      for (const el of area.querySelectorAll("*")) {
        const b = el.getBoundingClientRect();
        if (b.width > 0 && (b.right > box.right + 1 || b.left < box.left - 1)) found.push(`${name(el)} ${Math.round(b.left)}..${Math.round(b.right)} outside ${name(area)} ${Math.round(box.left)}..${Math.round(box.right)}`);
      }
    }
    return found;
  });
}

async function openSettings(page: Page, theme: Theme, lang: string | null) {
  await page.goto(fixtureUrl("dashpay-registered", theme, lang ? `&lang=${lang}` : ""));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await expect(page.getByTestId("settings-display")).toBeVisible();
  await expect(page.getByTestId("settings-dust")).toBeVisible();
  await expect(page.getByTestId("settings-privacy")).toBeVisible();
  await fontsReady(page);
}

const SHELL = ["toolbar", "sidebar", "status-bar"];

for (const theme of THEMES) {
  test(`the language picker saves the choice and the window follows it, ${theme}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await openSettings(page, theme, null);
    const picker = page.getByTestId("settings-language");
    await expect(picker).toHaveValue("");
    await expect(picker.locator("option:checked")).toHaveText("System default (English)");
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.locator("html")).toHaveAttribute("dir", "ltr");

    await picker.selectOption("en-XA");
    await expect(page.locator("html")).toHaveAttribute("lang", "en-XA");
    await expect(page.getByTestId("sidebar-home")).toHaveText(/^⟦Ĥöɱé·+⟧$/);
    await expect(page.getByTestId("settings-tabs-general")).toHaveText(/^⟦.+⟧$/);
    expect(await page.evaluate(() => window.__dwFixture!.world.settings.ui.language)).toBe("en-XA");

    await picker.selectOption("ar-XB");
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
    await expect(page.getByTestId("sidebar-home")).toHaveText("‮Home‬");
    // The sidebar is on the right, the page on its left.
    const sidebar = (await page.getByTestId("sidebar").boundingBox())!;
    const main = (await page.getByTestId("page-settings").boundingBox())!;
    expect(sidebar.x).toBeGreaterThan(main.x);

    await picker.selectOption("");
    await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
    await expect(page.getByTestId("sidebar-home")).toHaveText("Home");
    expect(await page.evaluate(() => window.__dwFixture!.world.settings.ui.language)).toBe(null);
    expect(errors).toEqual([]);
  });

  test(`Reset options puts the language back to the system's, ${theme}`, async ({ page }) => {
    await openSettings(page, theme, "en-XA");
    await page.getByTestId("settings-reset").click();
    await page.getByTestId("settings-reset-confirm").click();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByTestId("settings-language")).toHaveValue("");
  });

  for (const dir of ["ltr", "rtl"] as const) {
    test(`Settings tabs: the arrow keys move focus to the tab drawn on that side, ${dir}, ${theme}`, async ({ page }) => {
      await openSettings(page, theme, dir === "rtl" ? "ar-XB" : null);
      await expect(page.locator("html")).toHaveAttribute("dir", dir);
      const general = page.getByTestId("settings-tabs-general");
      const security = page.getByTestId("settings-tabs-security");
      const [toSecurity, back] = dir === "rtl" ? ["ArrowLeft", "ArrowRight"] : ["ArrowRight", "ArrowLeft"];
      const g = (await general.boundingBox())!;
      const s = (await security.boundingBox())!;
      expect(dir === "rtl" ? s.x < g.x : s.x > g.x, "Security is drawn on the side its key points to").toBe(true);
      await general.focus();
      await page.keyboard.press(toSecurity);
      await expect(security).toBeFocused();
      await expect(security).toHaveAttribute("aria-selected", "true");
      await page.keyboard.press(back);
      await expect(general).toBeFocused();
      await expect(general).toHaveAttribute("aria-selected", "true");
    });
  }

  for (const [lang, kind] of [
    ["en-XA", "long"],
    ["ar-XB", "rtl"],
  ] as const) {
    test(`the shell and Settings ▸ General show only catalog text under ${lang}, fit, and pass axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await openSettings(page, theme, lang);
        await expect(page.locator("html")).toHaveAttribute("dir", kind === "rtl" ? "rtl" : "ltr");
        expect(await untranslated(page, [...SHELL, "page-settings"], kind), `${lang} at ${size.width}`).toEqual([]);
        expect(await overflow(page), `${lang} at ${size.width}`).toEqual([]);
        if (size.width === 1280) {
          await page.screenshot({ path: artifactPath(info, `i18n-settings-${lang}-${theme}.png`) });
          expect(await seriousAxeViolations(page)).toEqual([]);
        }
      }
      expect(errors).toEqual([]);
    });

    test(`the lock screen shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await page.goto(fixtureUrl("funded", theme, `&lang=${lang}`));
        await page.getByTestId("toolbar-lock").click();
        await expect(page.getByTestId("lock-screen")).toBeVisible();
        await fontsReady(page);
        expect(await untranslated(page, ["lock-screen"], kind), `${lang} at ${size.width}`).toEqual([]);
        expect(await overflow(page), `${lang} at ${size.width}`).toEqual([]);
        if (size.width === 1280) {
          await page.screenshot({ path: artifactPath(info, `i18n-lock-${lang}-${theme}.png`) });
          expect(await seriousAxeViolations(page)).toEqual([]);
        }
      }
    });
  }
}

test("the untranslated-text check sees English that bypasses the catalog", async ({ page }) => {
  await openSettings(page, "light", null);
  const found = await untranslated(page, [...SHELL, "page-settings"], "long");
  for (const text of ["Home", "Settings", "Display", "Up to date"]) expect(found).toContain(text);
  expect(await untranslated(page, [...SHELL, "page-settings"], "rtl")).toContain("Home");
});

test("a username keeps its direction in a right-to-left window", async ({ page }) => {
  await openSettings(page, "light", "ar-XB");
  const chip = page.getByTestId("toolbar-chip").locator("bdi");
  await expect(chip).toHaveText("@alice");
});

test("an unknown ?lang= fails loudly", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", "light", "&lang=xx-YY"));
  await expect(page.locator("body")).toContainText("unknown language xx-YY", { timeout: 30_000 });
  expect(errors).toEqual([]);
});
