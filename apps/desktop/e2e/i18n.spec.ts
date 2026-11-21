import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO, untranslated } from "./pseudo";

// Localization (MP-14): the language picker in Settings ▸ General, and the first converted area (the
// shell, the lock screen and Settings ▸ General) under the two pseudo-locales (./pseudo.ts). Each
// later area has its own `i18n-<area>.spec.ts`.

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

  for (const pseudo of PSEUDO) {
    const [lang] = pseudo;
    test(`the shell and Settings ▸ General show only catalog text under ${lang}, fit, and pass axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, { name: "settings", roots: [...SHELL, "page-settings"], open: (lang) => openSettings(page, theme, lang) });
      expect(errors).toEqual([]);
    });

    test(`the lock screen shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      await checkArea(page, info, theme, pseudo, {
        name: "lock",
        roots: ["lock-screen"],
        open: async (lang) => {
          await page.goto(fixtureUrl("funded", theme, `&lang=${lang}`));
          await page.getByTestId("toolbar-lock").click();
          await expect(page.getByTestId("lock-screen")).toBeVisible();
        },
      });
    });
  }
}

test("the untranslated-text check sees English that bypasses the catalog", async ({ page }) => {
  await openSettings(page, "light", null);
  const found = await untranslated(page, [...SHELL, "page-settings"], "long");
  for (const text of ["Home", "Settings", "Display", "Up to date"]) expect(found).toContain(text);
  expect(await untranslated(page, [...SHELL, "page-settings"], "rtl")).toContain("Home");
});

for (const [saved, shown, expected] of [
  ["EN", "English", "en"],
  ["pt-BR", "pt-BR (not available, using System default)", "pt-BR"],
] as const) {
  test(`a saved language ${saved} shows as its own row and choosing System default clears it`, async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registered", "light", `&savedLang=${saved}`));
    await page.getByTestId("toolbar").waitFor();
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    const picker = page.getByTestId("settings-language");
    await expect(picker).toHaveValue(expected);
    await expect(picker.locator("option:checked")).toHaveText(shown);
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await picker.selectOption("");
    await expect(picker).toHaveValue("");
    expect(await page.evaluate(() => window.__dwFixture!.world.settings.ui.language)).toBe(null);
    await expect(picker.locator("option", { hasText: "not available" })).toHaveCount(0);
  });
}

test("a debit keeps its sign before the digits in a right-to-left window", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light", "&lang=ar-XB"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "activity" }));
  const debit = page.locator(".tx-row-amount .amount-number", { hasText: /^-/ }).first();
  await expect(debit).toBeVisible();
  // The drawn order, not the DOM's: the sign's box is left of the first digit's.
  const [sign, digit] = await debit.evaluate((el) => {
    const node = el.firstChild!;
    const box = (i: number) => {
      const r = document.createRange();
      r.setStart(node, i);
      r.setEnd(node, i + 1);
      return r.getBoundingClientRect().left;
    };
    return [box(0), box(1)];
  });
  expect(sign).toBeLessThan(digit);
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
