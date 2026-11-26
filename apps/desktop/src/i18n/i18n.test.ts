import { afterEach, describe, expect, it } from "vitest";
import { createElement, isValidElement, type ReactElement, type ReactNode } from "react";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { build, fill, flatten, pluralForm, pseudoCatalog, renderTag, tokenize, type Catalog, type Params } from "./core";
import { en } from "./en";
import { formatLocale, isComplete, LANGUAGES, resolveLanguage, runtimeFormatLocale, systemLanguage, t, type Language } from "./index";

const choose = (language: string | null) => ui.set({ prefs: { ...DEFAULT_PREFS, language } });
afterEach(() => choose(null));

/** The text a rich message renders to, with each element's children inlined in brackets. */
function text(node: ReactNode): string {
  if (Array.isArray(node)) return node.map(text).join("");
  if (isValidElement(node)) {
    const el = node as ReactElement<{ children?: ReactNode }>;
    const inner = text(el.props.children);
    return typeof el.type === "string" ? `[${el.type}:${inner}]` : inner;
  }
  return node === null || node === undefined || typeof node === "boolean" ? "" : String(node);
}

describe("t", () => {
  it("fills named placeholders and formats numbers for the locale", () => {
    expect(t("network.confirmTitle", { network: "Testnet" })).toBe("Switch to Testnet?");
    expect(fill("{a} of {b}", { a: 1234.5, b: "x" }, { locale: "de-DE", isolate: false })).toBe("1.234,5 of x");
    expect(fill("{a} of {b}", { a: 1234.5, b: "x" }, { locale: "en-US", isolate: false })).toBe("1,234.5 of x");
  });

  it("chooses the plural form by count through Intl.PluralRules", () => {
    expect(t("shell.status.peers", { count: 1 })).toBe("1 peer");
    expect(t("shell.status.peers", { count: 0 })).toBe("0 peers");
    expect(t("shell.status.peers", { count: 8 })).toBe("8 peers");
    const ru = { one: "{count} пир", few: "{count} пира", many: "{count} пиров", other: "{count} пира" };
    expect([1, 3, 5, 21, 1.5].map((n) => fill(pluralForm(ru, n, "ru"), { count: n }, { locale: "ru", isolate: false }))).toEqual(["1 пир", "3 пира", "5 пиров", "21 пир", "1,5 пира"]);
    expect(pluralForm({ other: "x" }, 1, "en")).toBe("x");
  });

  it("falls back to English for a key a language lacks", () => {
    const partial: Language = { tag: "xx", name: "X", dir: "ltr", messages: new Map([["shell.status.synced", "Synkd"]]) };
    LANGUAGES.push(partial);
    try {
      choose("xx");
      expect(t("shell.status.synced")).toBe("Synkd");
      expect(t("shell.status.offline")).toBe("Offline");
      expect(isComplete(partial)).toBe(false);
    } finally {
      LANGUAGES.pop();
    }
  });

  it("picks the plural form of an English fallback by English's rules, and formats the number in the language's locale", () => {
    const ru: Language = { tag: "ru-x-dwtest", name: "Русский", dir: "ltr", messages: new Map([["shell.status.synced", "Синхронизировано"]]) };
    LANGUAGES.push(ru);
    try {
      choose("ru-x-dwtest");
      // Russian puts 21 in `one`; the English text it falls back to does not.
      expect(t("shell.status.peers", { count: 21 })).toBe("21 peers");
      expect(t("shell.status.peers", { count: 1 })).toBe("1 peer");
      expect(t("shell.status.peers", { count: 1234 })).toBe(`${new Intl.NumberFormat("ru").format(1234)} peers`);
    } finally {
      LANGUAGES.pop();
    }
  });

  it("renders rich text: tags through their renderers, placeholders as elements, no concatenation", () => {
    const parts = build<string>("Read <b>the {doc}</b> and <link>open {n}</link>.", { doc: "terms", n: 2 }, { locale: "en", isolate: false }, (tag, c) => `[${tag}:${c.join("")}]`);
    expect(parts.join("")).toBe("Read [b:the terms] and [link:open 2].");
    expect(build<string>("<b>unclosed", {}, { locale: "en", isolate: false }, (tag, c) => `[${tag}:${c.join("")}]`).join("")).toBe("[b:unclosed]");
    const node = t.rich("settings.privacy.usernameInRequestsHint", { wallet: createElement("strong", null, "Main") });
    expect(text(node)).toContain("payment requests of “[bdi:[strong:Main]]” show");
    expect(tokenize("a {b} <c>d</c>")).toEqual([
      { kind: "text", text: "a " },
      { kind: "param", name: "b" },
      { kind: "text", text: " " },
      { kind: "open", tag: "c" },
      { kind: "text", text: "d" },
      { kind: "close", tag: "c" },
    ]);
  });

  it("finds tag renderers and values among own properties only", () => {
    const wrap = (values: Record<string, unknown>) => (tag: string, c: string[]) => {
      const r = renderTag<string>(values, tag, c);
      return Array.isArray(r) ? r.join("") : r;
    };
    const inherited = Object.create({ link: (c: string[]) => `[link:${c.join("")}]` }) as Record<string, unknown>;
    expect(build<string>("<link>open</link> <constructor>x</constructor>", inherited, { locale: "en", isolate: false }, wrap(inherited)).join("")).toBe("open x");
    const given = { link: (c: string[]) => `[link:${c.join("")}]` };
    expect(build<string>("<link>open</link>", given, { locale: "en", isolate: false }, wrap(given)).join("")).toBe("[link:open]");
    expect(fill("{toString}|{constructor}", {}, { locale: "en", isolate: false })).toBe("|");
  });

  it("types keys and parameters from the English catalog", () => {
    // @ts-expect-error: not a key
    expect(t("shell.status.nope")).toBe("shell.status.nope");
    // @ts-expect-error: the message takes `network`
    t("network.confirmTitle");
    // @ts-expect-error: a plural takes `count`
    t("shell.status.peers", {});
    // @ts-expect-error: a plural's `count` is a number
    expect(t("shell.status.peers", { count: "1" })).toBe("1 peers");
    // @ts-expect-error: no placeholder named `who`
    t("network.confirmTitle", { who: "x" });
    // A plural takes its other placeholders as well as `count`.
    const words = { one: "{name} has {count} word", other: "{name} has {count} words" } as const;
    const both: Params<typeof words, string> = { name: "Main", count: 2 };
    // @ts-expect-error: and still requires them
    const missing: Params<typeof words, string> = { count: 2 };
    expect(fill(pluralForm(words, 1, "en"), { ...both, count: 1 }, { locale: "en", isolate: false })).toBe("Main has 1 word");
    expect(missing).toBeDefined();
    const ok: Catalog<typeof en> = { shell: { status: { synced: "Synkd", peers: { one: "a", other: "b" } } } };
    // @ts-expect-error: a key English does not have fails tsc
    const extra: Catalog<typeof en> = { shell: { status: { syncd: "Synkd" } } };
    // @ts-expect-error: nor does an area that has no English messages yet
    const empty: Catalog<typeof en> = { pay: { rogue: "x" } };
    expect([ok, extra, empty]).toHaveLength(3);
  });
});

describe("the language in use", () => {
  const real = [LANGUAGES[0]];
  const es: Language = { tag: "es", name: "Español", dir: "ltr", messages: new Map(flatten(en)) };
  const ptPartial: Language = { tag: "pt-BR", name: "Português (Brasil)", dir: "ltr", messages: new Map([["routes.home", "Início"]]) };

  it("follows the system's first language with a complete catalog, by tag then by language", () => {
    const shipped = [...real, es, ptPartial];
    expect(systemLanguage(["es-MX", "en-US"], shipped).tag).toBe("es");
    expect(systemLanguage(["pt-BR", "es"], shipped).tag).toBe("es");
    expect(systemLanguage(["de-DE", "fr"], shipped).tag).toBe("en");
    expect(systemLanguage([], shipped).tag).toBe("en");
  });

  it("matches a language alone only in a script that does not conflict", () => {
    const hans: Language = { tag: "zh-Hans", name: "简体中文", dir: "ltr", messages: new Map(flatten(en)) };
    const shipped = [...real, hans];
    expect(systemLanguage(["zh-Hans-CN", "en"], shipped).tag).toBe("zh-Hans");
    expect(systemLanguage(["zh-CN"], shipped).tag).toBe("zh-Hans");
    expect(systemLanguage(["zh-Hant-TW", "en"], shipped).tag).toBe("en");
    expect(systemLanguage(["zh-TW", "zh-Hans"], shipped).tag).toBe("zh-Hans");
    expect(systemLanguage(["pt-PT"], [...real, { ...es, tag: "pt" }]).tag).toBe("pt");
  });

  it("never takes a pseudo-locale from the system", () => {
    expect(systemLanguage(["en-XA", "ar-XB"]).tag).toBe("en");
  });

  it("takes a saved choice the build ships, and follows the system for one it does not", () => {
    expect(resolveLanguage("en-XA", ["en"]).tag).toBe("en-XA");
    expect(resolveLanguage("ar-xb", ["en"]).dir).toBe("rtl");
    expect(resolveLanguage("qaa", ["en-GB"]).tag).toBe("en");
    expect(resolveLanguage(null, ["qaa-FR"]).tag).toBe("en");
  });

  it("formats with the base locale unless another language is chosen", () => {
    expect(formatLocale("en-US", null, ["de"])).toBe("en-US");
    expect(formatLocale(undefined, null, ["de"])).toBe(undefined);
    expect(formatLocale("en-US", "en", ["en-US"])).toBe("en-US");
    expect(formatLocale(undefined, "en", ["en-GB"])).toBe(undefined);
    expect(formatLocale(undefined, "en", ["de-DE"])).toBe("en");
    expect(formatLocale("en-US", "ar-XB", ["en-US"])).toBe("ar-XB");
    expect(formatLocale("en-US", "qaa", ["en-US"])).toBe("en-US");
    // A chosen language in another script than the system's formats in its own.
    LANGUAGES.push({ tag: "zh-Hans-x-dwtest", name: "简体中文", dir: "ltr", messages: new Map() });
    try {
      expect(formatLocale("zh-TW", "zh-Hans-x-dwtest", ["zh-TW"])).toBe("zh-Hans-x-dwtest");
      expect(formatLocale("zh-CN", "zh-Hans-x-dwtest", ["zh-CN"])).toBe("zh-CN");
    } finally {
      LANGUAGES.pop();
    }
  });

  it("drops ICU's POSIX variant from the runtime's default locale, and only that", () => {
    expect(runtimeFormatLocale("en-US-u-va-posix")).toBe("en-US");
    expect(runtimeFormatLocale("en-US-POSIX")).toBe("en-US");
    expect(runtimeFormatLocale("en-US")).toBe(undefined);
    expect(runtimeFormatLocale("de-DE-u-nu-latn")).toBe(undefined);
  });

  it("keeps a POSIX locale's language and its other keywords, so it formats as that locale", () => {
    expect(runtimeFormatLocale("de-DE-u-va-posix")).toBe("de-DE");
    expect(new Intl.NumberFormat(runtimeFormatLocale("de-DE-u-va-posix")).format(1596000)).toBe("1.596.000");
    expect(runtimeFormatLocale("en-US-u-nu-arab-va-posix")).toBe("en-US-u-nu-arab");
    expect(runtimeFormatLocale("en-US-u-va-posix-nu-arab")).toBe("en-US-u-nu-arab");
    expect(runtimeFormatLocale("ar-EG-u-nu-latn-va-posix")).toBe("ar-EG-u-nu-latn");
    expect(new Intl.NumberFormat(runtimeFormatLocale("ar-EG-u-nu-latn-va-posix")).format(1596000)).toMatch(/^1\D596\D000$/);
  });

  it("groups digits when the runtime's default is POSIX, as WebKitGTK's is under LANG=C.UTF-8", () => {
    const Real = Intl.NumberFormat;
    // WebKitGTK under LANG=C.UTF-8: navigator.languages is ["C"] and the default is en-US-u-va-posix,
    // which formats with no grouping; the same tag passed explicitly groups.
    function PosixDefault(locales?: string | string[], options?: Intl.NumberFormatOptions) {
      if (locales !== undefined) return new Real(locales, options);
      const nf = new Real("en-US", { ...options, useGrouping: false });
      nf.resolvedOptions = () => ({ ...Real.prototype.resolvedOptions.call(nf), locale: "en-US-u-va-posix" });
      return nf;
    }
    Intl.NumberFormat = PosixDefault as unknown as typeof Intl.NumberFormat;
    try {
      expect(t("onboarding.options.startsAt", { height: 1596000 })).toBe("The scan starts at block 1,596,000.");
      expect(formatLocale(undefined, null, ["C"])).toBe("en-US");
      expect(formatLocale(undefined, "en", ["C"])).toBe("en");
    } finally {
      Intl.NumberFormat = Real;
    }
    expect(t("onboarding.options.startsAt", { height: 1596000 })).toBe("The scan starts at block 1,596,000.");
    expect(formatLocale(undefined, null, ["C"])).toBe(undefined);
  });
});

describe("pseudo-locales", () => {
  const messages = flatten(en);
  const long = pseudoCatalog(messages, "long");
  const rtl = pseudoCatalog(messages, "rtl");

  it("cover every key, so they count as complete", () => {
    // Only the generated ones: a real translation may lag English and falls back to it per key.
    const pseudo = LANGUAGES.filter((l) => l.pseudo);
    expect(pseudo.map((l) => l.tag)).toEqual(["en-XA", "ar-XB"]);
    for (const l of pseudo) expect(isComplete(l)).toBe(true);
  });

  it("bracket, accent and lengthen every message, leaving placeholders and tags", () => {
    const m = long.get("network.confirmTitle") as string;
    expect(m.startsWith("⟦") && m.endsWith("⟧")).toBe(true);
    expect(m).toContain("{network}");
    expect(m).not.toContain("Switch");
    expect(m.replace(/\{\w+\}/g, "")).not.toMatch(/[A-Za-z]/);
    const literal = (x: string) => x.replace(/\{\w+\}/g, "").length;
    for (const [key, source] of messages)
      if (typeof source === "string") expect(literal(long.get(key) as string), key).toBeGreaterThanOrEqual(literal(source) * 1.3);
    expect(fill(m, { network: "Testnet" }, { locale: "en", isolate: false })).toContain("Testnet");
    expect(long.get("shell.status.peers")).toMatchObject({ one: expect.stringContaining("{count}"), other: expect.stringContaining("{count}") });
  });

  it("draw each literal run right to left and leave placeholder values as they read", () => {
    const m = rtl.get("network.confirmTitle") as string;
    expect(m).toBe("‮Switch to ‬{network}‮?‬");
  });

  it("keep a multi-paragraph message right to left in every paragraph", () => {
    const m = rtl.get("settings.about.license.text") as string;
    const paragraphs = m.split("\n").filter((p) => p !== "");
    expect(paragraphs.length).toBeGreaterThan(3);
    for (const p of paragraphs) expect(p.startsWith("‮") && p.endsWith("‬"), p.slice(0, 20)).toBe(true);
  });

  it("read through t once chosen", () => {
    choose("en-XA");
    expect(t("shell.status.synced")).toMatch(/^⟦Šýñçéð·+⟧$/);
    choose("ar-XB");
    expect(t.language.dir).toBe("rtl");
    expect(t("network.confirmTitle", { network: "@alice" })).toBe("\u202eSwitch to \u202c\u2068@alice\u2069\u202e?\u202c");
  });
});

describe("cached formatters and catalog checks", () => {
  it("follow each language switch, there and back", () => {
    const ru: Language = { tag: "ru-x-dwtest", name: "Русский", dir: "ltr", messages: new Map([["shell.status.peers", { one: "{count} пир", few: "{count} пира", many: "{count} пиров", other: "{count} пира" }]]) };
    const de: Language = { tag: "de-x-dwtest", name: "Deutsch", dir: "ltr", messages: new Map([["shell.status.peers", { one: "{count} Peer", other: "{count} Peers" }]]) };
    LANGUAGES.push(ru, de);
    try {
      const peers = () => [21, 5, 1234].map((count) => t("shell.status.peers", { count }));
      const english = peers();
      expect(english).toEqual(["21 peers", "5 peers", "1,234 peers"]);
      choose("ru-x-dwtest");
      const russian = peers();
      expect(russian).toEqual(["21 пир", "5 пиров", `${new Intl.NumberFormat("ru").format(1234)} пира`]);
      choose("de-x-dwtest");
      expect(peers()).toEqual(["21 Peers", "5 Peers", "1.234 Peers"]);
      choose(null);
      expect(peers()).toEqual(english);
      choose("ru-x-dwtest");
      expect(peers()).toEqual(russian);
    } finally {
      LANGUAGES.pop();
      LANGUAGES.pop();
    }
  });

  it("check a swapped-in catalog afresh", () => {
    const xx: Language = { tag: "xx", name: "X", dir: "ltr", messages: new Map([["shell.status.synced", "Synkd"]]) };
    expect(isComplete(xx)).toBe(false);
    xx.messages = new Map(flatten(en));
    expect(isComplete(xx)).toBe(true);
  });
});

describe("bidi isolation", () => {
  it("isolates a right-to-left value inside a left-to-right sentence, and leaves left-to-right values as they are", () => {
    expect(t("network.confirmTitle", { network: "Testnet" })).toBe("Switch to Testnet?");
    expect(t("network.confirmTitle", { network: "@سارة" })).toBe("Switch to \u2068@سارة\u2069?");
    expect(t("network.confirmTitle", { network: "@שרה" })).toBe("Switch to \u2068@שרה\u2069?");
    // An unterminated override in a name cannot reorder the rest of the sentence.
    expect(t("network.confirmTitle", { network: "@x\u202Dy" })).toBe("Switch to \u2068@x\u202Dy\u2069?");
    // A stray isolate end cannot close the isolate around the value, nor an unclosed start outlive it.
    expect(t("network.confirmTitle", { network: "x\u2069\u202Ey" })).toBe("Switch to \u2068\u2068x\u2069\u202Ey\u2069?");
    expect(t("network.confirmTitle", { network: "x\u2067y" })).toBe("Switch to \u2068x\u2067y\u2069\u2069?");
    // An isolate cannot span a paragraph separator, so each paragraph of a value is isolated alone.
    expect(t("network.confirmTitle", { network: "x\u2029\u202Ey" })).toBe("Switch to \u2068x\u2069\u2029\u2068\u202Ey\u2069?");
    expect(t("network.confirmTitle", { network: "\u2067x\r\ny" })).toBe("Switch to \u2068\u2067x\u2069\u2069\r\n\u2068y\u2069?");
    for (const sep of ["\n", "\r", "\x1C", "\x1D", "\x1E", "\x85"])
      expect(t("network.confirmTitle", { network: `x${sep}\u202Ey` }), JSON.stringify(sep)).toBe(`Switch to \u2068x\u2069${sep}\u2068\u202Ey\u2069?`);
    // Sogdian, past the first supplementary right-to-left block.
    expect(t("network.confirmTitle", { network: "\u{10F30}" })).toBe("Switch to \u2068\u{10F30}\u2069?");
    expect(t("network.confirmTitle", { network: "a\uFEFFb" })).toBe("Switch to a\uFEFFb?");
  });

  it("isolates an element placeholder of rich text in every language", () => {
    const name = createElement("b", null, "@سارة");
    choose("ar-XB");
    expect(text(t.rich("network.confirmTitle", { network: name }))).toBe("\u202eSwitch to \u202c[bdi:[b:@سارة]]\u202e?\u202c");
    choose(null);
    expect(text(t.rich("network.confirmTitle", { network: name }))).toBe("Switch to [bdi:[b:@سارة]]?");
    expect(text(t.rich("network.confirmTitle", { network: createElement("b", null, "@alice") }))).toBe("Switch to [bdi:[b:@alice]]?");
  });

  it("draws an English fallback left to right as a whole in a right-to-left language", () => {
    const ar: Language = { tag: "ar-x-dwtest", name: "العربية", dir: "rtl", messages: new Map([["shell.status.synced", "متزامن"]]) };
    LANGUAGES.push(ar);
    try {
      choose("ar-x-dwtest");
      expect(t("shell.status.synced")).toBe("متزامن");
      expect(t("shell.status.peers", { count: 21 })).toBe(`\u2066\u2068${new Intl.NumberFormat("ar").format(21)}\u2069 peers\u2069`);
      expect(text(t.rich("network.confirmTitle", { network: "x" }))).toBe("\u2066Switch to \u2068x\u2069?\u2069");
      // The left-to-right isolate cannot span a paragraph separator either, so it ends and starts again at one.
      expect(t("network.confirmTitle", { network: "x\u2029y" })).toBe("\u2066Switch to \u2068x\u2069\u2069\u2029\u2066\u2068y\u2069?\u2069");
      expect(text(t.rich("network.confirmTitle", { network: "x\u2029y" }))).toBe("\u2066Switch to \u2068x\u2069\u2069\u2029\u2066\u2068y\u2069?\u2069");
    } finally {
      LANGUAGES.pop();
    }
  });
});

describe("other catalogs", () => {
  /** Placeholders and tags a message uses. */
  const marks = (m: string | object) =>
    [...new Set((typeof m === "string" ? [m] : Object.values(m)).flatMap((s: string) => s.match(/\{\w+\}|<\/?\w+>/g) ?? []))].sort();

  it("use only English's placeholders and tags in each message", () => {
    const english = flatten(en);
    for (const l of LANGUAGES)
      for (const [key, m] of l.messages) {
        const source = english.get(key);
        expect(source, `${l.tag} ${key}`).toBeDefined();
        expect(marks(m).filter((x) => !marks(source!).includes(x)), `${l.tag} ${key}`).toEqual([]);
      }
  });
});
