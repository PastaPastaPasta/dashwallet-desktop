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
type Texts<M> = M extends string ? M : M extends Plural ? NonNullable<M[keyof Plural]> : never;

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
  return message[new Intl.PluralRules(language).select(count)] ?? message.other;
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
 * letters is isolated, so English text stays as it was. */
export interface Fill {
  locale: string | undefined;
  isolate: boolean;
}

/** Hebrew, Arabic, Syriac, Thaana, NKo, Samaritan, Mandaic and their presentation forms, and the
 * right-to-left marks and overrides. */
const RTL = /[\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFF\u200F\u202B\u202E\u2067]|\uD802[\uDC00-\uDFFF]|\uD803[\uDC00-\uDE7F]|\uD83A[\uDC00-\uDFFF]|\uD83B[\uDC00-\uDEFF]/;

export const hasRtl = (text: string) => RTL.test(text);

/** `message` with its placeholders filled. Tags are dropped, their text kept. */
export function fill(message: string, values: Values, how: Fill): string {
  return tokenize(message)
    .map((t) => (t.kind === "text" ? t.text : t.kind === "param" ? show(own(values, t.name), how) : ""))
    .join("");
}

export function show(value: unknown, { locale, isolate }: Fill): string {
  const text = typeof value === "number" ? new Intl.NumberFormat(locale).format(value) : String(value ?? "");
  return isolate || hasRtl(text) ? `\u2068${text}\u2069` : text;
}

/** Wraps a tag's children with the renderer given for it: an own property of `values` that is a
 * function. Any other tag (an inherited name such as `constructor`, or none given) keeps its
 * children unwrapped. */
export function renderTag<N>(values: Values, tag: string, children: (N | string)[]): N | (N | string)[] {
  const render = own(values, tag);
  return typeof render === "function" ? (render as (c: (N | string)[]) => N)(children) : children;
}

/** A rich-text message as a tree: text, filled placeholders, and each tag's children handed to
 * its renderer. An element given for a placeholder goes through `isolateNode` when `how.isolate`
 * is on, so it keeps its own direction as a text value does. Generic over the node type so it
 * stays free of React. */
export function build<N>(message: string, values: Values, how: Fill, wrap: (tag: string, children: (N | string)[]) => N, isolateNode: (node: N) => N = (n) => n): (N | string)[] {
  const stack: { tag: string; children: (N | string)[] }[] = [{ tag: "", children: [] }];
  for (const t of tokenize(message)) {
    const top = stack[stack.length - 1];
    if (t.kind === "text") top.children.push(t.text);
    else if (t.kind === "param") {
      const v = own(values, t.name);
      top.children.push(typeof v === "string" || typeof v === "number" || v === undefined || v === null ? show(v, how) : how.isolate ? isolateNode(v as N) : (v as N));
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
function rtlText(message: string): string {
  return tokenize(message)
    .map((t) => (t.kind === "text" ? `‮${t.text}‬` : t.kind === "param" ? `{${t.name}}` : `<${t.kind === "close" ? "/" : ""}${t.tag}>`))
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
