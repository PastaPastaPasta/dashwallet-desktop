import { expect, type JSHandle, type Page } from "@playwright/test";

// What a screen reader is given by a live region, recorded in the page.

/** How `el` reads to assistive tech now, in the page: "<politeness>: <text>" (polite or assertive)
 * for an exposed live region; for a live region a reader cannot reach yet, why not and its text;
 * else "absent" or "not a live region". An explicit `aria-live` wins over the role's default (`off`
 * is not live). A region inside aria-busy="true" is not exposed: readers hold its updates. The
 * text is what a reader gets: text inside an aria-hidden, hidden, inert or unrendered element of
 * the region is left out. Self-contained: it runs in the page. */
export function liveState(el: Element | null): string {
  if (!el?.isConnected) return "absent";
  const explicit = el.getAttribute("aria-live");
  const politeness = explicit !== null ? explicit.trim() : ({ status: "polite", alert: "assertive", log: "polite" } as Record<string, string>)[el.getAttribute("role") ?? ""];
  if (politeness !== "polite" && politeness !== "assertive") return "not a live region";
  const parts: string[] = [];
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    let shown = true;
    for (let p = node.parentElement; p && p !== el; p = p.parentElement) {
      const style = getComputedStyle(p);
      if (p.getAttribute("aria-hidden") === "true" || p.hasAttribute("hidden") || p.hasAttribute("inert") || style.display === "none" || style.visibility === "hidden") {
        shown = false;
        break;
      }
    }
    if (shown) parts.push((node as Text).data);
  }
  const text = parts.join("").replace(/\s+/g, " ").trim();
  if (el.closest('[aria-hidden="true"], [inert]')) return `hidden from assistive tech: ${text}`;
  if (el.closest('[aria-busy="true"]')) return `busy, so readers hold its updates: ${text}`;
  const modal = document.querySelector('[aria-modal="true"]');
  if (modal && !modal.contains(el)) return `outside the open modal: ${text}`;
  if (!el.checkVisibility({ visibilityProperty: true, checkVisibilityCSS: true })) return `not rendered: ${text}`;
  return `${politeness}: ${text}`;
}

type Heard = Element & { __heard?: string[] };

/** Records what `el` says from now on: at each content change that changes its text for a reader,
 * `liveState` then. A reader announces changed text only, so text that arrived while the region
 * was hidden and was exposed later, and a mutation that leaves the text as it was, say nothing new.
 * Self-contained: it runs in the page, after `install`. */
function record(el: Heard) {
  const heard: string[] = [];
  el.__heard = heard;
  const read = (window as unknown as { __dwLiveState: typeof liveState }).__dwLiveState;
  const textOf = (state: string) => state.slice(state.indexOf(": ") + 2);
  let last = textOf(read(el));
  new MutationObserver(() => {
    const state = read(el);
    if (textOf(state) === last) return;
    last = textOf(state);
    heard.push(state);
  }).observe(el, { childList: true, subtree: true, characterData: true });
}

type InPage = { __dwLiveState: typeof liveState; __dwRecord: typeof record; __dwAdded?: Heard & { __added: string } };

const install = (page: Page) => page.evaluate(`window.__dwLiveState = ${liveState.toString()}; window.__dwRecord = ${record.toString()}`);

/** `record` for `region` from now on. A missing `el` records nothing. */
export async function armRecorder(page: Page, region: JSHandle<Element | null>) {
  await install(page);
  await region.evaluate((el: Heard | null) => {
    if (el) (window as unknown as InPage).__dwRecord(el);
  });
}

export const heardBy = (region: JSHandle<Element | null>) => region.evaluate((el: Heard | null) => el?.__heard ?? []);

/** The live region at `selector`, held and recorded before an action, once it is there as a live
 * region (empty, with `empty`), whether or not a reader can reach it yet: what counts is that it
 * is exposed when it speaks. */
export async function liveRegion(page: Page, selector: string, { empty = true } = {}) {
  await install(page);
  const state = () => page.evaluate((s) => (window as unknown as { __dwLiveState: typeof liveState }).__dwLiveState(document.querySelector(s)), selector);
  const kinds = "polite|assertive|hidden from assistive tech|busy, so readers hold its updates|outside the open modal|not rendered";
  await expect.poll(state, { message: `live region ${selector} before the action` }).toMatch(new RegExp(empty ? `^(${kinds}): $` : `^(${kinds}): `));
  const region = await page.evaluateHandle((s) => document.querySelector(s), selector);
  await armRecorder(page, region);
  return region;
}

/** The element held by `liveRegion` said `text` while exposed, at `politeness`: all its text, or
 * (a RegExp) a match in it. */
export async function expectSaid(region: JSHandle<Element | null>, politeness: "polite" | "assertive", text: string | RegExp) {
  const said = typeof text === "string" ? `${politeness}: ${text}` : expect.stringMatching(new RegExp(`^${politeness}: .*${text.source}`));
  await expect.poll(() => heardBy(region)).toContainEqual(said);
}

/** For a region that comes with its screen: watches for the first element at `selector` to enter
 * the page, and `record`s it from then on. The returned function gives that element and its
 * `liveState` as it entered. */
export async function regionWhenAdded(page: Page, selector: string) {
  await install(page);
  await page.evaluate((s) => {
    const w = window as unknown as InPage;
    if (document.querySelector(s)) throw new Error(`${s} is already in the page; regionWhenAdded watches for its insertion`);
    w.__dwAdded = undefined;
    const watch = new MutationObserver(() => {
      const el = document.querySelector<Heard & { __added: string }>(s);
      if (!el) return;
      watch.disconnect();
      el.__added = w.__dwLiveState(el);
      w.__dwRecord(el);
      w.__dwAdded = el;
    });
    watch.observe(document.body, { childList: true, subtree: true });
  }, selector);
  return async () => {
    await page.waitForFunction(() => (window as unknown as InPage).__dwAdded !== undefined, null, { timeout: 10_000 });
    const region = (await page.evaluateHandle(() => (window as unknown as InPage).__dwAdded ?? null)) as JSHandle<Element | null>;
    return { region, added: await region.evaluate((el) => (el as Heard & { __added: string }).__added) };
  };
}
