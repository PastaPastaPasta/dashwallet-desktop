import { expect, test, type Locator, type Page } from "@playwright/test";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Home under the two pseudo-locales (./pseudo.ts): the balance hero in each
// sync state, its breakdown and Core line, discreet mode, the shortcuts, the History header and its
// loading, failed and hidden states, the protocol advisory banner, and the Unfinished transfers card
// and sheet. The history rows themselves are Activity's and stay out of these roots.

/** Formatted Dash amounts ("0.50 tDASH", "30.19698287", "#.##"), and the sync line's empty state. */
const DATA = [/^[+-]?[\p{Nd}.,٫٬\s•*#]+( (t?DASH|mDASH|μDASH|duffs))?$/u, /^[—…]$/, /^[0-9a-f]+…[0-9a-f]+$/];

/** Engine text, not translated in 1.0 (`errorMessage`): the failed read's message. */
const ENGINE = [/^fixture: /];

const BAND = ["hero-band"];
const HISTORY = ["history-header"];

async function openHome(page: Page, scenario: string, theme: Theme, lang: string, extra = "") {
  await page.goto(fixtureUrl(scenario, theme, `${extra}&lang=${lang}`));
  await page.getByTestId("hero").waitFor();
}

/** Sets the first wallet's balances and tells the page. */
async function setCore(page: Page, over: Record<string, number>) {
  await page.evaluate((over) => {
    const f = window.__dwFixture!;
    const w = f.world.wallets[0];
    w.summary = { ...w.summary, balances: { ...w.summary.balances!, ...over } };
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: w.summary.walletId, balances: w.summary.balances });
  }, over);
}

async function stopSync(page: Page) {
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.sync = { ...f.world.sync, running: false, caughtUp: false, activePhase: null, connectedPeers: 0 };
    f.emit({ kind: "sync", network: f.world.launch.network.dirName, status: f.world.sync });
  });
}

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  data?: RegExp[];
  go?: (page: Page) => Promise<void>;
}

const HELD = { unconfirmed: 20_000_000, immature: 200_000_000, locked: 1_000_000_000 };

async function focusCoreDetail(page: Page) {
  const core = page.getByTestId("breakdown-core");
  await expect(core).toHaveAttribute("tabindex", "0");
  await core.focus();
}

export const STATES: Record<string, State> = {
  funded: { scenario: "funded", roots: [...BAND, ...HISTORY, "see-all"], go: (page) => page.getByTestId("see-all").waitFor() },
  "core-only": { scenario: "pools-absent", roots: BAND, go: (page) => expect(page.getByTestId("breakdown-core").locator(".breakdown-subtitle")).toContainText("0.20") },
  "core-held": {
    scenario: "funded",
    roots: BAND,
    go: async (page) => {
      await setCore(page, HELD);
      await focusCoreDetail(page);
      await expect(page.getByRole("tooltip")).toContainText(/·.*·/);
    },
  },
  "core-only-held": {
    scenario: "pools-absent",
    roots: BAND,
    go: async (page) => {
      await setCore(page, HELD);
      await focusCoreDetail(page);
      await expect(page.getByRole("tooltip")).toContainText(/·.*·/);
    },
  },
  syncing: { scenario: "catching-up-payment", roots: [...BAND, ...HISTORY], go: (page) => expect(page.getByTestId("hero-sync-line")).not.toHaveText("", { timeout: 15_000 }) },
  stopped: {
    scenario: "empty-activity",
    roots: [...BAND, ...HISTORY],
    go: async (page) => {
      await stopSync(page);
      await expect(page.getByTestId("hero-sync-line")).not.toHaveText("", { timeout: 15_000 });
    },
  },
  discreet: {
    scenario: "funded",
    roots: [...BAND, ...HISTORY, "history-hidden"],
    go: async (page) => {
      await page.getByTestId("toolbar-discreet").click();
      await page.getByTestId("history-hidden").waitFor();
    },
  },
  unavailable: { scenario: "funded", extra: "&fail=balance:none", roots: BAND, go: (page) => expect(page.getByTestId("hero-amount")).toHaveClass(/hero-unknown/) },
  "pool-failed": { scenario: "funded", extra: "&fail=platform_addresses:none", roots: BAND, go: (page) => expect(page.getByTestId("breakdown-platform")).toContainText("—") },
  "pool-loading": { scenario: "funded", extra: "&hold=shielded_status", roots: BAND, go: (page) => expect(page.getByTestId("breakdown-shielded")).toContainText("…") },
  "history-failed": { scenario: "funded", extra: "&fail=history_page:none", roots: [...HISTORY, "history-error"], data: ENGINE, go: (page) => page.getByTestId("history-error").waitFor() },
  "history-loading": { scenario: "funded", extra: "&hold=history_page", roots: [...HISTORY, "history-loading"], go: (page) => page.getByTestId("history-loading").waitFor() },
  advisory: { scenario: "protocol-upgraded", roots: ["protocol-advisory-banner"], go: (page) => page.getByTestId("protocol-advisory-banner").waitFor() },
  "advisory-open-failed": {
    scenario: "protocol-upgraded",
    extra: "&fail=open_releases_page:none",
    roots: ["protocol-advisory-banner"],
    go: async (page) => {
      await page.getByTestId("protocol-advisory-link").click();
      await page.getByTestId("protocol-advisory-error").waitFor();
    },
  },
  "unfinished-card": { scenario: "recovery-restored-mixed", roots: ["unfinished-card"], go: (page) => page.getByTestId("unfinished-text").waitFor() },
  "unfinished-sheet": {
    scenario: "recovery-restored-mixed",
    roots: ["unfinished-sheet"],
    go: async (page) => {
      await page.getByTestId("unfinished-details").click();
      await expect(page.getByTestId("unfinished-row")).toHaveCount(3);
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Home ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `home-${name}`,
          roots: state.roots,
          data: [...DATA, ...(state.data ?? [])],
          open: async (lang) => {
            await openHome(page, state.scenario, theme, lang, state.extra);
            await state.go?.(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("the shortcuts follow a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await openHome(page, "funded", "light", "en");
  await expect(page.getByTestId("shortcut-transfer")).toHaveText("Transfer");
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-language").selectOption("en-XA");
  await page.getByTestId("sidebar-home").click();
  for (const id of ["receive", "send", "transfer", "backup"]) await expect(page.getByTestId(`shortcut-${id}`)).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("shortcut-backup")).toHaveAttribute("title", /^⟦.+⟧$/);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-language").selectOption("");
  await page.getByTestId("sidebar-home").click();
  await expect(page.getByTestId("shortcut-transfer")).toHaveText("Transfer");
  expect(errors).toEqual([]);
});

/** The left edges of `re`'s two groups in the first text node under `root` that matches it. */
async function lefts(root: Locator, re: RegExp): Promise<[number, number] | null> {
  return root.evaluate(
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

type Span = { left: number; right: number; top: number; bottom: number };

/** The edges of the first match of `re` in the text under `root`. */
async function span(root: Locator, re: RegExp): Promise<Span | null> {
  return root.evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const m = re.exec(n.textContent ?? "");
        if (!m) continue;
        const r = document.createRange();
        r.setStart(n, m.index);
        r.setEnd(n, m.index + m[0].length);
        const rects = [...r.getClientRects()];
        return {
          left: Math.min(...rects.map((x) => x.left)),
          right: Math.max(...rects.map((x) => x.right)),
          top: Math.min(...rects.map((x) => x.top)),
          bottom: Math.max(...rects.map((x) => x.bottom)),
        };
      }
      return null;
    },
    [re.source, re.flags] as const,
  );
}

/** In RTL prose, `first` precedes `second` when it sits on an earlier line or to `second`'s right on the same line. */
function expectBeforeInRtl(first: Span, second: Span, message: string) {
  const earlierLine = first.bottom <= second.top;
  const sameLine = first.top < second.bottom && second.top < first.bottom;
  expect(earlierLine || (sameLine && first.left > second.right), message).toBe(true);
}

/** In RTL prose, an amount sits immediately to the right of its word on the same line. */
function expectBesideWordInRtl(amount: Span, word: Span, message: string) {
  expect(amount.top, message).toBeLessThan(word.bottom);
  expect(word.top, message).toBeLessThan(amount.bottom);
  expect(amount.left - word.right, message).toBeLessThan(12);
  expect(amount.left - word.right, message).toBeGreaterThanOrEqual(0);
}

test.describe("amounts keep their order under ar-XB", () => {
  test("the Core line keeps each amount beside its word, the first part rightmost", async ({ page }) => {
    await openHome(page, "pools-absent", "light", "ar-XB");
    await setCore(page, HELD);
    await focusCoreDetail(page);
    const subtitle = page.getByTestId("breakdown-core").locator(".breakdown-subtitle");
    await expect(subtitle).toContainText("0.20");
    const pending = await span(subtitle, /0\.20/);
    const pendingWord = await span(subtitle, /gnidnep|pending/);
    expect(pending, "the pending amount on the Core row").not.toBeNull();
    expect(pendingWord, "the pending word on the Core row").not.toBeNull();
    expectBesideWordInRtl(pending!, pendingWord!, "the pending amount sits beside its word");

    const note = page.getByRole("tooltip");
    await expect(note).toContainText(/·.*·/);
    const pendingStart = await span(note, /demrifnocnU|Unconfirmed/);
    const pendingEnd = await span(note, /sdnes|sends/);
    const immature = await span(note, /2\.00/);
    const immatureWord = await span(note, /erutammi|immature/);
    const locked = await span(note, /10\.00/);
    const lockedWord = await span(note, /dekcol|locked/);
    expect(pendingStart, "the pending detail starts in the tooltip").not.toBeNull();
    expect(pendingEnd, "the pending detail ends in the tooltip").not.toBeNull();
    expect(immature, "the immature amount in the tooltip").not.toBeNull();
    expect(immatureWord, "the immature word in the tooltip").not.toBeNull();
    expect(locked, "the locked amount in the tooltip").not.toBeNull();
    expect(lockedWord, "the locked word in the tooltip").not.toBeNull();
    expectBeforeInRtl(pendingStart!, immature!, "the pending detail is first, before immature");
    expectBeforeInRtl(pendingEnd!, immature!, "the pending detail ends to the right of immature");
    expect(pendingStart!.right, "the pending detail starts rightmost in the tooltip").toBeGreaterThan(immature!.right);
    expectBeforeInRtl(immature!, locked!, "immature's part comes before locked's");
    expectBesideWordInRtl(immature!, immatureWord!, "the immature amount sits beside its word");
    expectBesideWordInRtl(locked!, lockedWord!, "the locked amount sits beside its word");
  });


  test("an Unfinished transfers row's amount comes before its unit", async ({ page }) => {
    await openHome(page, "recovery-restored-mixed", "light", "ar-XB");
    await page.getByTestId("unfinished-details").click();
    const amount = page.getByTestId("unfinished-row-amount").first();
    await expect(amount).toContainText(/\p{Nd}/u);
    const at = await lefts(amount, /(\p{Nd})[\p{Nd}.,٫٬]* (t?DASH)/u);
    expect(at, "the row amount").not.toBeNull();
    expect(at![0], "the row amount").toBeLessThan(at![1]);
  });
});
