// The message catalog's mechanics (MP-14), free of React and of the app's state: the catalog
// types, key lookup, placeholders, plural forms, rich-text tags and the pseudo-locales.
// docs/design/LOCALIZATION.md says how screens use them.

/** A message with plural forms: `other` always, the rest as the language's
 * `Intl.PluralRules` names them. Chosen by the `count` parameter. */
export interface Plural {
  readonly zero?: string;
  readonly one?: string;
  readonly two?: string;
  readonly few?: string;
  readonly many?: string;
  readonly other: string;
}

export type Message = string | Plural;
export interface Tree {
  readonly [key: string]: Tree | Message;
}

const isPlural = (m: Tree | Message): m is Plural => typeof m === "object" && typeof (m as { other?: unknown }).other === "string";

/** The dotted keys of a catalog's messages. */
export type KeyOf<T, P extends string = ""> = {
  [K in keyof T & string]: T[K] extends Message ? `${P}${K}` : KeyOf<T[K], `${P}${K}.`>;
}[keyof T & string];

/** The message at `K`. */
export type At<T, K extends string> = K extends `${infer H}.${infer R}` ? (H extends keyof T ? At<T[H], R> : never) : K extends keyof T ? T[K] : never;

type Placeholders<S> = S extends `${string}{${infer P}}${infer R}` ? P | Placeholders<R> : never;
type Tags<S> = S extends `${string}<${infer T}>${infer R}` ? (T extends `/${string}` ? Tags<R> : T | Tags<R>) : never;
type Texts<M> = M extends string ? M : M extends Plural ? NonNullable<M[keyof M & keyof Plural]> : never;

/** The named placeholders a message takes; a plural also takes `count`. */
export type ParamNames<M> = Placeholders<Texts<M>> | (M extends Plural ? "count" : never);
/** The tags a rich-text message wraps text in. */
export type TagNames<M> = Tags<Texts<M>>;
/** The values a message takes, each of type `V`; a plural's `count` is a number. */
export type Params<M, V> = { [P in ParamNames<M>]: P extends "count" ? (M extends Plural ? number : V) : V };

/** Another language's catalog: any of English's keys, no other (an extra or mistyped key fails
 * `tsc`, and an area English has no messages in yet takes none); a missing key falls back to
 * English. */
export type Catalog<T> = { readonly [K in keyof T]?: T[K] extends string ? string : T[K] extends Plural ? Plural : [keyof T[K]] extends [never] ? never : Catalog<T[K]> };

/** Every message of `tree` by its dotted key. */
export function flatten(tree: Tree, prefix = "", into = new Map<string, Message>()): Map<string, Message> {
  for (const [k, v] of Object.entries(tree)) {
    if (typeof v === "string" || isPlural(v)) into.set(prefix + k, v);
    else flatten(v, `${prefix}${k}.`, into);
  }
  return into;
}

export type Values = Record<string, unknown>;

/** The value given for `name`: an own property of `values`, never an inherited one. */
export function own(values: Values, name: string): unknown {
  return Object.prototype.hasOwnProperty.call(values, name) ? values[name] : undefined;
}

/** The form of `message` for `count` in `language`. */
export function pluralForm(message: Message, count: unknown, language: string): string {
  if (typeof message === "string") return message;
  if (typeof count !== "number") return message.other;
  return message[pluralRules(language).select(count)] ?? message.other;
}

const pluralRuleCache = new Map<string, Intl.PluralRules>();
const numberFormatCache = new Map<string | undefined, Intl.NumberFormat>();

function pluralRules(language: string): Intl.PluralRules {
  let rules = pluralRuleCache.get(language);
  if (!rules) pluralRuleCache.set(language, (rules = new Intl.PluralRules(language)));
  return rules;
}

/** A number formatter for `locale`, built once: building one is slow in WebKit. */
export function numberFormat(locale: string | undefined): Intl.NumberFormat {
  let format = numberFormatCache.get(locale);
  if (!format) numberFormatCache.set(locale, (format = new Intl.NumberFormat(locale)));
  return format;
}

/** A message split into literal text, `{placeholder}`s and `<tag>`/`</tag>`s. */
export type Token = { kind: "text"; text: string } | { kind: "param"; name: string } | { kind: "open" | "close"; tag: string };

const TOKEN = /\{(\w+)\}|<(\/?)(\w+)>/g;

export function tokenize(message: string): Token[] {
  const out: Token[] = [];
  let at = 0;
  for (const m of message.matchAll(TOKEN)) {
    if (m.index > at) out.push({ kind: "text", text: message.slice(at, m.index) });
    if (m[1] !== undefined) out.push({ kind: "param", name: m[1] });
    else out.push({ kind: m[2] ? "close" : "open", tag: m[3] });
    at = m.index + m[0].length;
  }
  if (at < message.length) out.push({ kind: "text", text: message.slice(at) });
  return out;
}

/** How placeholders are filled: numbers formatted for `locale`; with `isolate` (a right-to-left
 * language) each value is a bidi isolate (U+2068 … U+2069), so a name or an address keeps its own
 * direction inside the sentence. In a left-to-right language only a value with right-to-left
 * letters or a bidi control is isolated, so English text stays as it was. */
export interface Fill {
  locale: string | undefined;
  isolate: boolean;
}

/** Hebrew, Arabic, Syriac, Thaana, NKo, Samaritan, Mandaic, the right-to-left scripts of the
 * supplementary planes and their presentation forms, the right-to-left mark, and every bidi
 * embedding, override and isolate control, so a value carrying an unterminated one cannot reorder
 * the rest of the sentence. */
const RTL = /[\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFE\u200F\u202A-\u202E\u2066-\u2069]|\uD802[\uDC00-\uDFFF]|\uD803[\uDC00-\uDFFF]|\uD83A[\uDC00-\uDFFF]|\uD83B[\uDC00-\uDEFF]/;

export const hasRtl = (text: string) => RTL.test(text);

/** `message` with its placeholders filled. Tags are dropped, their text kept. */
export function fill(message: string, values: Values, how: Fill): string {
  return tokenize(message)
    .map((t) => (t.kind === "text" ? t.text : t.kind === "param" ? show(own(values, t.name), how) : ""))
    .join("");
}

export function show(value: unknown, { locale, isolate }: Fill): string {
  const text = typeof value === "number" ? numberFormat(locale).format(value) : String(value ?? "");
  return isolate || hasRtl(text) ? isolated(text) : text;
}

/** A bidi paragraph separator: an isolate cannot span one. */
const PARAGRAPH = /(\r\n|[\n\r\x1C-\x1E\x85\u2029])/;

/** `text` with `close` before and `open` after each paragraph separator, so an isolate opened
 * around it is closed at the end of each paragraph and opened again in the next. */
export const acrossParagraphs = (text: string, open: string, close: string) =>
  text
    .split(PARAGRAPH)
    .map((part, i) => (i % 2 ? `${close}${part}${open}` : part))
    .join("");

/** `text` with each of its paragraphs isolated on its own, so no control in a later paragraph can
 * reach the text after it. */
function isolated(text: string): string {
  return text
    .split(PARAGRAPH)
    .map((part, i) => (i % 2 ? part : `\u2068${balanced(part)}\u2069`))
    .join("");
}

/** `text` with its isolates paired, so a stray U+2069 cannot close the isolate around it and an
 * unclosed U+2066–U+2068 cannot carry on past it. */
function balanced(text: string): string {
  let open = 0;
  let unopened = 0;
  for (const c of text) {
    if (c >= "\u2066" && c <= "\u2068") open++;
    else if (c === "\u2069") open > 0 ? open-- : unopened++;
  }
  return "\u2068".repeat(unopened) + text + "\u2069".repeat(open);
}

/** Wraps a tag's children with the renderer given for it: an own property of `values` that is a
 * function. Any other tag (an inherited name such as `constructor`, or none given) keeps its
 * children unwrapped. */
export function renderTag<N>(values: Values, tag: string, children: (N | string)[]): N | (N | string)[] {
  const render = own(values, tag);
  return typeof render === "function" ? (render as (c: (N | string)[]) => N)(children) : children;
}

/** A rich-text message as a tree: text, filled placeholders, and each tag's children handed to
 * its renderer. An element given for a placeholder goes through `isolateNode` in every language
 * (its text, unlike a string's, is not known here), so it keeps its own direction. Generic over the node type so it
 * stays free of React. */
export function build<N>(message: string, values: Values, how: Fill, wrap: (tag: string, children: (N | string)[]) => N, isolateNode: (node: N) => N = (n) => n): (N | string)[] {
  const stack: { tag: string; children: (N | string)[] }[] = [{ tag: "", children: [] }];
  for (const t of tokenize(message)) {
    const top = stack[stack.length - 1];
    if (t.kind === "text") top.children.push(t.text);
    else if (t.kind === "param") {
      const v = own(values, t.name);
      top.children.push(typeof v === "string" || typeof v === "number" || v === undefined || v === null ? show(v, how) : isolateNode(v as N));
    } else if (t.kind === "open") stack.push({ tag: t.tag, children: [] });
    else if (stack.length > 1 && top.tag === t.tag) {
      stack.pop();
      stack[stack.length - 1].children.push(wrap(t.tag, top.children));
    }
  }
  while (stack.length > 1) {
    const open = stack.pop()!;
    stack[stack.length - 1].children.push(wrap(open.tag, open.children));
  }
  return stack[0].children;
}

// ----------------------------------------------------------------------------- pseudo-locales

const ACCENTED: Record<string, string> = Object.fromEntries(
  [..."abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"].map((c, i) => [c, [..."åƀçðéƒĝĥîĵķļɱñöþǫŕšţûṽŵẋýžÅƁÇÐÉƑĜĤÎĴĶĻṀÑÖÞǪŔŠŢÛṼŴẊÝŽ"][i]]),
);

/** The long pseudo-locale's text: accented, 40% longer, bracketed, so a string that bypasses
 * the catalog shows as plain ASCII and a cut-off one loses its closing bracket. */
function longText(message: string): string {
  let letters = 0;
  const body = tokenize(message)
    .map((t) => {
      if (t.kind === "param") return `{${t.name}}`;
      if (t.kind !== "text") return `<${t.kind === "close" ? "/" : ""}${t.tag}>`;
      letters += t.text.length;
      return [...t.text].map((c) => ACCENTED[c] ?? c).join("");
    })
    .join("");
  return `⟦${body}${"·".repeat(Math.ceil(letters * 0.4))}⟧`;
}

/** The right-to-left pseudo-locale's text: each literal run drawn right to left (U+202E … U+202C),
 * placeholders left as their values read. */
/** The RTL pseudo-locale: every run of text right to left, in every paragraph of it (an override
 * ends at a paragraph separator). */
function rtlText(message: string): string {
  return tokenize(message)
    .map((t) => (t.kind === "text" ? `‮${acrossParagraphs(t.text, "‮", "‬")}‬` : t.kind === "param" ? `{${t.name}}` : `<${t.kind === "close" ? "/" : ""}${t.tag}>`))
    .join("");
}

export type PseudoKind = "long" | "rtl";

/** A pseudo-locale's catalog, made from English. */
export function pseudoCatalog(messages: Map<string, Message>, kind: PseudoKind): Map<string, Message> {
  const f = kind === "long" ? longText : rtlText;
  const out = new Map<string, Message>();
  for (const [k, m] of messages) out.set(k, typeof m === "string" ? f(m) : (Object.fromEntries(Object.entries(m).map(([form, s]) => [form, f(s)])) as unknown as Plural));
  return out;
}
