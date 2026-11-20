// The renderer's languages (MP-14): the shipped catalogs, the language in use (Settings ▸ General's
// choice, else the system's when a complete catalog exists for it, else English), `t` and
// `useT`, and the `<html lang dir>` and formatting locale that follow it.

import { Fragment, useLayoutEffect, useMemo, type ReactNode } from "react";
import { ui, useUi } from "../app/ui";
import { build, fill, flatten, own, pluralForm, pseudoCatalog, renderTag, type At, type KeyOf, type Message, type Params, type ParamNames, type TagNames } from "./core";
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

const byTag = (tag: string) => LANGUAGES.find((l) => l.tag.toLowerCase() === tag.toLowerCase());
const primary = (tag: string) => tag.split("-")[0].toLowerCase();

export const isLanguage = (tag: string) => byTag(tag) !== undefined;

/** A catalog with every English key. */
export const isComplete = (l: Language) => [...english.keys()].every((k) => l.messages.has(k));

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
 * language alone ("pt-PT" takes "pt"); English otherwise. */
export function systemLanguage(system: readonly string[], languages: Language[] = LANGUAGES): Language {
  const usable = languages.filter((l) => !l.pseudo && isComplete(l));
  for (const s of system) {
    const hit = usable.find((l) => l.tag.toLowerCase() === s.toLowerCase()) ?? usable.find((l) => primary(l.tag) === primary(s));
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

/** The locale numbers, dates and fiat are formatted with, for `base` (the system's, `undefined`,
 * or the fixture's): `base` while the language follows the system or is the system's own
 * (its regional formats stay), the chosen language's otherwise. */
export function formatLocale(base: string | undefined, choice: string | null = ui.get().prefs.language, system: readonly string[] = systemLanguages()): string | undefined {
  const chosen = choice === null ? undefined : byTag(choice);
  if (!chosen) return base;
  const own = base ?? system[0];
  return own !== undefined && primary(own) === primary(chosen.tag) ? base : chosen.tag;
}

type Args<K extends Key> = [ParamNames<At<typeof en, K>>] extends [never] ? [] : [values: Params<At<typeof en, K>, string | number>];
type RichArgs<K extends Key> = [values: Params<At<typeof en, K>, ReactNode> & Record<TagNames<At<typeof en, K>>, (children: ReactNode) => ReactNode>];

function lookup(language: Language, key: string, values: Record<string, unknown>): string {
  const message = language.messages.get(key) ?? english.get(key) ?? key;
  return pluralForm(message, own(values, "count"), language.tag);
}

function translate(language: Language, locale: string | undefined, key: string, values: Record<string, unknown> = {}): string {
  return fill(lookup(language, key, values), values, { locale, isolate: language.dir === "rtl" });
}

function translateRich(language: Language, locale: string | undefined, key: string, values: Record<string, unknown>): ReactNode {
  const parts = build<ReactNode>(lookup(language, key, values), values, { locale, isolate: language.dir === "rtl" }, (tag, children) => renderTag<ReactNode>(values, tag, children) as ReactNode);
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
