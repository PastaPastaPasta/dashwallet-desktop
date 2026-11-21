import { expect, type Page, type TestInfo } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fontsReady, type Theme } from "./helpers";

// The pseudo-locale checks (MP-14) every converted area runs: `en-XA` brackets, accents and
// lengthens every catalog string, so text that bypasses the catalog stays plain ASCII and a cut-off
// string loses its bracket; `ar-XB` turns the window right to left.

export type PseudoKind = "long" | "rtl";

export const PSEUDO = [
  ["en-XA", "long"],
  ["ar-XB", "rtl"],
] as const;

export const SIZES = [
  { width: 1280, height: 820 },
  // The window's minimum (dw-app tauri.conf.json).
  { width: 920, height: 600 },
];

/** Text that is data, not copy: the fixture's username, its avatar initial and wallet name, units and paths. */
export const DATA = [/^@[\w.-]+$/, /^\p{Lu}$/u, /^Main$/, /^(t?DASH|mDASH|μDASH|duffs)$/, /^\/[\w./-]+$/];

/** The visible text in `areas` that bypasses the catalog: plain Latin text that is no catalog
 * string's and is not data. A root that is not on the page is reported, so a check never passes
 * by scanning nothing. */
export async function untranslated(page: Page, areas: string[], kind: PseudoKind, data: RegExp[] = []): Promise<string[]> {
  const found = await page.evaluate(
    ({ areas, kind }) => {
      const out: string[] = [];
      for (const testId of areas) {
        const root = document.querySelector(`[data-testid="${testId}"]`);
        if (!root) {
          out.push(`missing root [data-testid="${testId}"]`);
          continue;
        }
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
  return found.filter((text) => ![...DATA, ...data].some((re) => re.test(text)));
}

/** The panes whose descendants must stay inside them: the shell, the page and the lock column. */
export const PANES = ['[data-testid="toolbar"]', '[data-testid="sidebar"]', '[data-testid="status-bar"]', "main.page", '[data-testid="lock-column"]'];

/** What scrolls sideways or sticks out of the window or its pane. */
export function overflow(page: Page, panes: string[] = PANES) {
  return page.evaluate((panes) => {
    const found: string[] = [];
    const doc = document.scrollingElement!;
    if (doc.scrollWidth > doc.clientWidth) found.push(`document ${doc.scrollWidth} > ${doc.clientWidth}`);
    const name = (el: Element) => `${el.tagName.toLowerCase()}${el.getAttribute("data-testid") ? `[${el.getAttribute("data-testid")}]` : ""}.${[...el.classList].join(".")}`;
    for (const el of document.querySelectorAll<HTMLElement>("body *")) {
      const x = getComputedStyle(el).overflowX;
      if ((x === "auto" || x === "scroll") && el.scrollWidth > el.clientWidth + 1) found.push(`${name(el)} scrolls ${el.scrollWidth} > ${el.clientWidth}`);
    }
    for (const area of document.querySelectorAll(panes.join(", "))) {
      const box = area.getBoundingClientRect();
      for (const el of area.querySelectorAll("*")) {
        const b = el.getBoundingClientRect();
        if (b.width > 0 && (b.right > box.right + 1 || b.left < box.left - 1)) found.push(`${name(el)} ${Math.round(b.left)}..${Math.round(b.right)} outside ${name(area)} ${Math.round(box.left)}..${Math.round(box.right)}`);
      }
    }
    return found;
  }, panes);
}

export interface AreaCheck {
  /** Opens the state under `lang` (pass it as `&lang=` to `fixtureUrl`) and waits until it is drawn. */
  open: (lang: string) => Promise<void>;
  /** `data-testid`s of the roots whose text is checked. */
  roots: string[];
  /** Screenshot file name stem: `i18n-<name>-<lang>-<theme>.png`. */
  name: string;
  /** Text in these roots that is data, not copy. */
  data?: RegExp[];
  /** Selectors of further panes (a sheet, a dialog) whose descendants must stay inside them, on top
   * of `PANES`. */
  panes?: string[];
}

/** Runs `check.open` at both window sizes under `lang`, and fails on text that bypasses the catalog,
 * on sideways overflow, and (at 1280×820, where it also takes the screenshot) on serious axe
 * violations. */
export async function checkArea(page: Page, info: TestInfo, theme: Theme, [lang, kind]: (typeof PSEUDO)[number], check: AreaCheck) {
  for (const size of SIZES) {
    await page.setViewportSize(size);
    await check.open(lang);
    await expect(page.locator("html")).toHaveAttribute("dir", kind === "rtl" ? "rtl" : "ltr");
    await fontsReady(page);
    expect(await untranslated(page, check.roots, kind, check.data), `${lang} at ${size.width}`).toEqual([]);
    for (const pane of check.panes ?? []) await expect(page.locator(pane).first(), `pane ${pane} at ${size.width}`).toBeVisible();
    expect(await overflow(page, [...PANES, ...(check.panes ?? [])]), `${lang} at ${size.width}`).toEqual([]);
    if (size.width === 1280) {
      await page.screenshot({ path: artifactPath(info, `i18n-${check.name}-${lang}-${theme}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
    }
  }
}
