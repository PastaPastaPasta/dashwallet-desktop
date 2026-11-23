// The renderer's languages (MP-14): the shipped catalogs, the language in use (Settings ▸ General's
// choice, else the system's when a complete catalog exists for it, else English), `t` and
// `useT`, and the `<html lang dir>` and formatting locale that follow it.

import { Fragment, useLayoutEffect, useMemo, type ReactNode } from "react";
import { ui, useUi } from "../app/ui";
import { acrossParagraphs, build, fill, flatten, own, pluralForm, pseudoCatalog, renderTag, type At, type KeyOf, type Message, type Params, type ParamNames, type TagNames } from "./core";
import { en } from "./en";

export type { Catalog } from "./core";
export type Key = KeyOf<typeof en>;

export interface Language {
  /** BCP 47 tag, as Settings saves it. */
  tag: string;
  /** Its name in its own language: the picker shows it untranslated. */
  name: string;
  dir: "ltr" | "rtl";
  messages: Map<string, Message>;
  /** Made from English for tests and the fixture build only; never a system match. */
  pseudo?: boolean;
}

const english = flatten(en);

/** The languages this build ships, English first. */
export const LANGUAGES: Language[] = [
  { tag: "en", name: "English", dir: "ltr", messages: english },
  // `typeof`: e2e specs load this module in Node, where Vite's define does not run.
  ...(typeof __DW_FIXTURES__ !== "undefined" && __DW_FIXTURES__
    ? [
        { tag: "en-XA", name: "English (pseudo, long)", dir: "ltr", messages: pseudoCatalog(english, "long"), pseudo: true } as const,
        { tag: "ar-XB", name: "Right to left (pseudo)", dir: "rtl", messages: pseudoCatalog(english, "rtl"), pseudo: true } as const,
      ]
    : []),
];

/** The shipped language with this tag, compared case-insensitively. */
export const byTag = (tag: string) => LANGUAGES.find((l) => l.tag.toLowerCase() === tag.toLowerCase());
const primary = (tag: string) => tag.split("-")[0].toLowerCase();

const scripts = new Map<string, string | undefined>();

/** The script a tag is written in, given or implied (`zh-TW` is Hant), if the runtime knows it. */
function script(tag: string): string | undefined {
  if (scripts.has(tag)) return scripts.get(tag);
  let found: string | undefined;
  try {
    found = new Intl.Locale(tag).maximize().script;
  } catch {
    found = undefined;
  }
  scripts.set(tag, found);
  return found;
}

/** The same language in a script that does not conflict: `pt-PT` takes `pt`, `zh-Hans-CN` takes
 * `zh-Hans`, `zh-Hant-TW` does not. */
function sameLanguage(catalog: string, wanted: string): boolean {
  if (primary(catalog) !== primary(wanted)) return false;
  const [a, b] = [script(catalog), script(wanted)];
  return a === undefined || b === undefined || a === b;
}

export const isLanguage = (tag: string) => byTag(tag) !== undefined;

const completeness = new WeakMap<Map<string, Message>, boolean>();

/** A catalog with every English key. A catalog does not change once built, so each is checked once:
 * `t` asks on every call. */
export function isComplete(l: Language): boolean {
  let complete = completeness.get(l.messages);
  if (complete === undefined) completeness.set(l.messages, (complete = [...english.keys()].every((k) => l.messages.has(k))));
  return complete;
}

/** The system's languages, most preferred first. */
export function systemLanguages(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages?.length ? navigator.languages : navigator.language ? [navigator.language] : [];
}

/** The system's locale for its region: its first language that names a region, else the runtime's
 * default locale. Independent of the language chosen in Settings. */
export function systemRegionLocale(system: readonly string[] = systemLanguages()): string | undefined {
  const withRegion = system.find((s) => {
    try {
      return new Intl.Locale(s).region !== undefined;
    } catch {
      return false;
    }
  });
  return withRegion ?? new Intl.DateTimeFormat().resolvedOptions().locale;
}

/** The system's first language that a complete, real catalog exists for, by tag and then by
 * language alone in a script that does not conflict ("pt-PT" takes "pt", "zh-Hant-TW" never takes
 * "zh-Hans"); English otherwise. */
export function systemLanguage(system: readonly string[], languages: Language[] = LANGUAGES): Language {
  const usable = languages.filter((l) => !l.pseudo && isComplete(l));
  for (const s of system) {
    const hit = usable.find((l) => l.tag.toLowerCase() === s.toLowerCase()) ?? usable.find((l) => sameLanguage(l.tag, s));
    if (hit) return hit;
  }
  return languages[0];
}

/** The language in use for a saved choice: a shipped one as chosen; `null` or one this build does
 * not ship follows the system. */
export function resolveLanguage(choice: string | null, system: readonly string[] = systemLanguages()): Language {
  return (choice !== null && byTag(choice)) || systemLanguage(system);
}

export const currentLanguage = (): Language => resolveLanguage(ui.get().prefs.language);

/** The runtime's default formatting locale: `undefined` (the runtime's own) unless that is ICU's
 * POSIX variant, which WebKitGTK takes from LANG=C.UTF-8 and which formats numbers with no digit
 * grouping; then the same locale without it, its other keywords (`nu`, `ca`, …) kept. */
export function runtimeFormatLocale(resolved: string = new Intl.NumberFormat().resolvedOptions().locale): string | undefined {
  if (!resolved.split("-").some((s) => s.toLowerCase() === "posix")) return undefined;
  let canonical: string;
  try {
    canonical = new Intl.Locale(resolved).toString();
  } catch {
    return "en";
  }
  const kept = canonical.replace(/-va-posix(?=-|$)/i, "").replace(/-u(?=-.(?:-|$)|$)/i, "");
  return kept === canonical ? undefined : kept;
}

let runtimeDefault: { from: typeof Intl.NumberFormat; locale: string | undefined } | undefined;
function runtimeDefaultLocale(): string | undefined {
  if (runtimeDefault?.from !== Intl.NumberFormat) runtimeDefault = { from: Intl.NumberFormat, locale: runtimeFormatLocale() };
  return runtimeDefault.locale;
}

/** The locale numbers, dates and fiat are formatted with, for `base` (the system's, `undefined`,
 * or the fixture's): `base` while the language follows the system or is the system's own in a
 * script that does not conflict (its regional formats stay), the chosen language's otherwise.
 * The system's is the runtime's default, `undefined`, unless that is ICU's POSIX variant. */
export function formatLocale(base: string | undefined, choice: string | null = ui.get().prefs.language, system: readonly string[] = systemLanguages()): string | undefined {
  const chosen = choice === null ? undefined : byTag(choice);
  const own = base ?? runtimeDefaultLocale();
  if (!chosen) return own;
  const ownLanguage = base ?? system[0];
  return ownLanguage !== undefined && sameLanguage(chosen.tag, ownLanguage) ? own : chosen.tag;
}

type Args<K extends Key> = [ParamNames<At<typeof en, K>>] extends [never] ? [] : [values: Params<At<typeof en, K>, string | number>];
type RichArgs<K extends Key> = [values: Params<At<typeof en, K>, ReactNode> & Record<TagNames<At<typeof en, K>>, (children: ReactNode) => ReactNode>];

/** The message for `key`, in its plural form chosen by the rules of the catalog that supplied it:
 * English's forms by English's rules when the language lacks the key. `fallback` says English
 * stood in for a right-to-left language, so the whole message is drawn left to right. */
function lookup(language: Language, key: string, values: Record<string, unknown>): { text: string; fallback: boolean } {
  const translated = language.messages.get(key);
  if (translated !== undefined) return { text: pluralForm(translated, own(values, "count"), language.tag), fallback: false };
  return { text: pluralForm(english.get(key) ?? key, own(values, "count"), "en"), fallback: language.dir === "rtl" };
}

/** Left-to-right isolate (U+2066 … U+2069). */
const LRI = "\u2066";
const PDI = "\u2069";

function translate(language: Language, locale: string | undefined, key: string, values: Record<string, unknown> = {}): string {
  const { text, fallback } = lookup(language, key, values);
  const filled = fill(text, values, { locale, isolate: language.dir === "rtl" });
  return fallback ? `${LRI}${acrossParagraphs(filled, LRI, PDI)}${PDI}` : filled;
}

function translateRich(language: Language, locale: string | undefined, key: string, values: Record<string, unknown>): ReactNode {
  const { text, fallback } = lookup(language, key, values);
  const built = build<ReactNode>(text, values, { locale, isolate: language.dir === "rtl" }, (tag, children) => renderTag<ReactNode>(values, tag, children) as ReactNode, (node) => <bdi>{node}</bdi>);
  const parts = fallback ? [LRI, ...built.map((p) => (typeof p === "string" ? acrossParagraphs(p, LRI, PDI) : p)), PDI] : built;
  return parts.map((p, i) => <Fragment key={i}>{p}</Fragment>);
}

export interface Translate {
  <K extends Key>(key: K, ...args: Args<K>): string;
  /** A message with tags (`<link>…</link>`) rendered by the components given for them, and
   * placeholders that may be elements. */
  rich<K extends Key>(key: K, ...args: RichArgs<K>): ReactNode;
  language: Language;
}

function translator(language: Language): Translate {
  const locale = formatLocale(undefined);
  const f = ((key: string, values?: Record<string, unknown>) => translate(language, locale, key, values)) as Translate;
  f.rich = ((key: string, values: Record<string, unknown>) => translateRich(language, locale, key, values)) as Translate["rich"];
  f.language = language;
  return f;
}

/** The message `key` in the language in use now. Outside React, or in a text module read during
 * a render; a component calls `useT` so it renders again when the language changes. */
export const t = ((key: string, values?: Record<string, unknown>) => translate(currentLanguage(), formatLocale(undefined), key, values)) as Translate;
t.rich = ((key: string, values: Record<string, unknown>) => translateRich(currentLanguage(), formatLocale(undefined), key, values)) as Translate["rich"];
Object.defineProperty(t, "language", { get: currentLanguage });

/** `t` for the language in use; the component renders again when it changes. */
export function useT(): Translate {
  const choice = useUi((s) => s.prefs.language);
  return useMemo(() => translator(resolveLanguage(choice)), [choice]);
}

/** Mirrors the language into `<html lang dir>`. Call once. */
export function useLanguageAttributes() {
  const { language } = useT();
  useLayoutEffect(() => {
    document.documentElement.lang = language.tag;
    document.documentElement.dir = language.dir;
  }, [language]);
}

/** A table whose values are catalog messages under `prefix`, read in the language in use. */
export function messageTable<K extends string>(prefix: string, keys: readonly K[]): Readonly<Record<K, string>> {
  const table = {} as Record<K, string>;
  for (const k of keys) Object.defineProperty(table, k, { enumerable: true, get: () => translate(currentLanguage(), formatLocale(undefined), `${prefix}.${k}`) });
  return table;
}
