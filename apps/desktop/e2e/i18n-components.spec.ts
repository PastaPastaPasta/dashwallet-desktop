import { expect, test, type Page } from "@playwright/test";
import { flatten, pseudoCatalog } from "../src/i18n/core";
import { en } from "../src/i18n/en";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// The shared controls under the pseudo-locales (MP-14): the unlock sheet's passphrase field, its
// busy button and Close; Settings ▸ General's sheets, their Close and the question's Cancel; a copy
// button's name, its Copied label and the toast; a transaction's date and time.

const LONG = pseudoCatalog(flatten(en), "long");
const long = (key: string) => LONG.get(key) as string;

async function unlockSheet(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("locked", theme, `&lang=${lang}`));
  await page.getByTestId("lock-mixing-only").check();
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await page.getByTestId("status-unlock").click();
  await expect(page.getByTestId("unlock-sheet-field")).toBeFocused();
  await fontsReady(page);
}

async function settings(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("funded", theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await expect(page.getByTestId("settings-display")).toBeVisible();
}

async function resetSheet(page: Page, theme: Theme, lang: string) {
  await settings(page, theme, lang);
  await page.getByTestId("settings-reset").click();
  await expect(page.getByTestId("settings-reset-sheet")).toBeVisible();
  await fontsReady(page);
}

async function governanceAsk(page: Page, theme: Theme, lang: string) {
  await settings(page, theme, lang);
  await page.getByTestId("settings-show-masternodes-on").click();
  await expect(page.getByTestId("settings-governance-ask")).toBeVisible();
  await fontsReady(page);
}

test("the unlock sheet names its field, Close and busy button from the catalog", async ({ page }) => {
  const errors = trackPageErrors(page);
  await unlockSheet(page, "light", "en-XA");
  const sheet = page.getByTestId("unlock-sheet");
  await expect(sheet.getByRole("button", { name: long("components.sheet.close"), exact: true })).toBeVisible();
  await expect(sheet.getByLabel(long("components.passphrase.passphrase"), { exact: true })).toBeVisible();
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("unlock"));
  await page.getByTestId("unlock-sheet-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("unlock-sheet-submit")).toHaveText(long("components.passphrase.checking"));
  await release.evaluate((r) => r());
  await expect(sheet).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("Settings ▸ General's sheets name Close and Cancel from the catalog", async ({ page }) => {
  await resetSheet(page, "light", "en-XA");
  await expect(page.getByTestId("settings-reset-sheet").getByRole("button", { name: long("components.sheet.close"), exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByTestId("settings-show-masternodes-on").click();
  const ask = page.getByTestId("settings-governance-ask");
  await expect(ask.getByRole("button", { name: long("components.sheet.close"), exact: true })).toBeVisible();
  await expect(ask.getByRole("button", { name: long("common.cancel"), exact: true })).toBeFocused();
});

test("a copy button's name and its toast come from the catalog", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async () => undefined } });
  });
  await settings(page, "light", "en-XA");
  const button = page.getByTestId("settings-data-dir").getByRole("button", { name: long("settings.general.dataDirectoryCopy"), exact: true });
  await button.click();
  await expect(button.locator(".copy-icon-check")).toHaveText("✓");
  await expect(page.locator(".toast")).toHaveText(long("components.copy.copied"));
});

test("the transaction table's caption, headers and status come from the catalog", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light", "&lang=en-XA"));
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-mode-table").click();
  const table = page.getByTestId("tx-table");
  await expect(table.locator("caption")).toHaveText(long("components.history.caption"));
  for (const key of ["status", "type", "party"]) await expect(table.locator("thead th", { hasText: long(`components.history.${key}`) })).toHaveCount(1);
  await expect(table.getByTestId("tx-sort-date")).toContainText(long("components.history.date"));
});

async function emptyHistory(page: Page, theme: Theme, lang: string) {
  await page.goto(fixtureUrl("empty", theme, `&lang=${lang}`));
  await expect(page.getByTestId("history-empty-receive")).toBeVisible();
  await fontsReady(page);
}

for (const theme of THEMES)
  for (const pseudo of PSEUDO) {
    const [lang] = pseudo;
    test(`the unlock sheet shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      await checkArea(page, info, theme, pseudo, { name: "unlock-sheet", roots: ["unlock-sheet"], open: (lang) => unlockSheet(page, theme, lang) });
    });

    test(`Settings ▸ General's reset sheet shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      await checkArea(page, info, theme, pseudo, { name: "reset-sheet", roots: ["settings-reset-sheet"], open: (lang) => resetSheet(page, theme, lang) });
    });

    test(`the governance question shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      await checkArea(page, info, theme, pseudo, { name: "governance-ask", roots: ["settings-governance-ask"], open: (lang) => governanceAsk(page, theme, lang) });
    });

    test(`an empty-state card shows only catalog text under ${lang}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      await checkArea(page, info, theme, pseudo, { name: "empty-history", roots: ["history-empty"], open: (lang) => emptyHistory(page, theme, lang) });
    });
  }

async function txDetail(page: Page, lang: string) {
  await page.goto(fixtureUrl("funded", "light", `&lang=${lang}`));
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  await page.locator(".tx-row").first().click();
  await expect(page.getByTestId("tx-detail-when")).toBeVisible();
  await fontsReady(page);
}

/** The left edges of the first characters of `re`'s two groups in `root`'s first matching text node. */
async function lefts(page: Page, testId: string, re: RegExp): Promise<[number, number] | null> {
  return page.getByTestId(testId).evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const m = re.exec(n.textContent ?? "");
        if (!m?.indices) continue;
        const left = (g: number) => {
          const r = document.createRange();
          r.setStart(n!, m.indices![g]![0]);
          r.setEnd(n!, m.indices![g]![0] + 1);
          return r.getBoundingClientRect().left;
        };
        return [left(1), left(2)] as [number, number];
      }
      return null;
    },
    [re.source, `${re.flags}d`] as const,
  );
}

test("a transaction's date and time are joined by the catalog's pattern under en-XA", async ({ page }) => {
  await txDetail(page, "en-XA");
  const when = page.getByTestId("tx-detail-when");
  await expect(when).toHaveText(/^⟦.*\p{Nd}{4}.*\p{Nd}{1,2}:\p{Nd}{2}.*⟧$/u);
  await expect(when).not.toContainText(" at ");
});

test("a transaction's date and time keep their order under ar-XB", async ({ page }) => {
  await txDetail(page, "ar-XB");
  const when = page.getByTestId("tx-detail-when");
  await expect(when).toContainText("\u202e");
  await expect(when).toHaveText(/^\u2068[^\u2069]*\p{Nd}{4}[^\u2069]*\u2069.*\u2068[^\u2069]*\p{Nd}{1,2}:\p{Nd}{2}[^\u2069]*\u2069$/u);
  const year = await lefts(page, "tx-detail-when", /(\p{Nd})\p{Nd}{2}(\p{Nd})/u);
  expect(year, "the year's first and last digits").not.toBeNull();
  expect(year![0]).toBeLessThan(year![1]);
  const time = await lefts(page, "tx-detail-when", /(\p{Nd})\p{Cf}*:\p{Cf}*\p{Nd}(\p{Nd})/u);
  expect(time, "the hour and the minutes").not.toBeNull();
  expect(time![0]).toBeLessThan(time![1]);
});
