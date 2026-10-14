// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The sample wallet's chain data: the data of docs/design/ux/home-*.png plus a month of
// history. Deterministic (seeded PRNG, fixed clock), so screenshots and element boxes are
// comparable across engines and runs.

import { COIN } from "../format/amount";
import type { Balances } from "../api/generated/Balances";
import type { AppError } from "../api/generated/AppError";
import type { HistoryRequest } from "../api/generated/HistoryRequest";
import type { HistoryRows } from "../api/generated/HistoryRows";
import type { HistorySortName } from "../api/generated/HistorySortName";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { TxCategoryName } from "../api/generated/TxCategoryName";
import type { TxDetails } from "../api/generated/TxDetails";
import type { TxRow } from "../api/generated/TxRow";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import type { TxStatusKindName } from "../api/generated/TxStatusKindName";
import type { TxTypeName } from "../api/generated/TxTypeName";
import type { WatchOnlyFilterName } from "../api/generated/WatchOnlyFilterName";

/** `default`: the reference wallet; `xss`: hostile labels and names (U6); `empty`: no history;
 * `large`: 10,000 transactions (paging and render cost). */
export type HistoryVariant = "default" | "xss" | "empty" | "large";

/** Fixed "now": Tuesday 6 October 2026, 12:00 local time. */
export const FIXTURE_NOW = new Date(2026, 9, 6, 12, 0, 0);

export const XSS_PAYLOADS = {
  label: `<img src=x onerror="window.__xss='label'">`,
  walletName: `<script>window.__xss='name'</script>Main`,
  contact: `"><svg onload="window.__xss='contact'">`,
};

const TIP = 1_352_000;

function prng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function makeIds(rand: () => number) {
  const hex = (n: number) => Array.from({ length: n }, () => "0123456789abcdef"[Math.floor(rand() * 16)]).join("");
  const address = () => "y" + Array.from({ length: 33 }, () => BASE58[Math.floor(rand() * 58)]).join("");
  return { hex, address };
}

export const secs = (d: Date) => Math.floor(d.getTime() / 1000);
const at = (daysAgo: number, h: number, m: number) =>
  new Date(FIXTURE_NOW.getFullYear(), FIXTURE_NOW.getMonth(), FIXTURE_NOW.getDate() - daysAgo, h, m);

const settled = (confirmations: number): TxStatusInfo => ({
  kind: confirmations >= 6 ? "confirmed" : "confirming",
  confirmations,
  instantLocked: true,
  chainLocked: true,
  maturesIn: null,
});

interface Spec {
  when: Date;
  type: TxTypeName;
  category: TxCategoryName;
  amount: number;
  status: TxStatusInfo;
  label?: string | null;
  fee?: number | null;
}

export function buildRows(variant: HistoryVariant): TxRow[] {
  if (variant === "empty") return [];
  const rand = prng(variant === "large" ? 1010 : 20261006);
  const { hex, address } = makeIds(rand);
  const specs: Spec[] = [
    {
      when: at(0, 1, 40),
      type: "recvWithAddress",
      category: "received",
      amount: 20_000_000,
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    },
    {
      when: at(0, 1, 10),
      type: "recvWithAddress",
      category: "received",
      amount: 125_000_000,
      label: "Coffee refund",
      status: { kind: "unconfirmed", confirmations: 0, instantLocked: true, chainLocked: false, maturesIn: null },
    },
    { when: at(1, 5, 46), type: "sendToAddress", category: "sent", amount: -50_000_226, fee: 226, label: "Alice", status: settled(150) },
  ];
  const count = variant === "large" ? 10_000 : 40;
  const kinds: [TxTypeName, TxCategoryName, number][] = [
    ["recvWithAddress", "received", 1],
    ["sendToAddress", "sent", -1],
    ["coinJoinMixing", "coinJoin", 0],
    ["sendToSelf", "internalTransfer", 0],
    ["recvWithAddress", "reward", 1],
  ];
  const labels = [null, null, null, "Rent", "Groceries", "Payroll", null, "Bob", null, "Exchange withdrawal"];
  for (let i = 0; i < count; i++) {
    const [type, category, sign] = kinds[Math.floor(rand() * kinds.length)];
    const daysAgo = 2 + Math.floor((i * (variant === "large" ? 900 : 30)) / count);
    const value = Math.floor(rand() * 4 * COIN) + Math.floor(rand() * 100_000);
    const fee = sign <= 0 ? 226 + Math.floor(rand() * 400) : null;
    specs.push({
      when: at(daysAgo, Math.floor(rand() * 24), Math.floor(rand() * 60)),
      type,
      category,
      amount: sign === 0 ? -(fee ?? 0) : sign * value,
      fee,
      label: type === "coinJoinMixing" || type === "sendToSelf" ? null : labels[Math.floor(rand() * labels.length)],
      status: settled(100 + i * 6),
    });
  }
  if (variant !== "large") {
    specs.push({
      when: at(3, 9, 15),
      type: "generated",
      category: "reward",
      amount: 288_000_000,
      status: { kind: "immature", confirmations: 40, instantLocked: false, chainLocked: true, maturesIn: 61 },
    });
    specs.push({
      when: at(4, 18, 2),
      type: "sendToAddress",
      category: "sent",
      amount: -12_345_678,
      fee: 374,
      status: { kind: "abandoned", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    });
  }
  if (variant === "xss") {
    specs[0].label = XSS_PAYLOADS.label;
    specs[2].label = XSS_PAYLOADS.contact;
  }
  const rows = specs.map((s) => {
    const confirmations = s.status.confirmations;
    return {
      txid: hex(64),
      recordIndex: 0,
      txType: s.type,
      category: s.category,
      status: s.status,
      timestamp: secs(s.when),
      blockHeight: confirmations > 0 ? TIP - confirmations + 1 : null,
      amount: s.amount,
      fee: s.fee ?? null,
      address: address(),
      label: s.label ?? null,
      countsTowardBalance: s.status.kind !== "abandoned",
      involvesWatchOnly: false,
    } satisfies TxRow;
  });
  return rows.sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
}

/** The reference wallet's balances (docs/design/ux/home-*.png). */
export const FUNDED_BALANCES: Balances = {
  confirmed: 3_019_698_287,
  unconfirmed: 20_000_000,
  immature: 0,
  locked: 0,
  total: 3_039_698_287,
  coinjoin: 0,
};

export const ZERO_BALANCES: Balances = { confirmed: 0, unconfirmed: 0, immature: 0, locked: 0, total: 0, coinjoin: 0 };

/** Caught up at the fixture tip. */
export function caughtUp(): SyncStatus {
  return {
    running: true,
    phases: (["headers", "filterHeaders", "filters", "masternodes"] as const).map((phase) => ({
      phase,
      currentHeight: TIP,
      targetHeight: TIP,
      done: true,
    })),
    activePhase: null,
    tipHeight: TIP,
    tipTime: secs(FIXTURE_NOW) - 90,
    chainlockHeight: TIP,
    connectedPeers: 8,
    caughtUp: true,
    secondsSinceProgress: null,
  };
}

// ---------------------------------------------------------------------------
// history_page: dw_engine::history::page over DTO rows (rust/crates/dw-engine/src/history.rs).
// Checked case by case against vectors/history.json, which dw-app generates from the engine
// (history_vectors_tests.rs) and CI regenerates; history.test.ts replays it.
// ---------------------------------------------------------------------------

/** Largest page (`MAX_PAGE`). */
const MAX_PAGE = 500;
/** Longest search text, in Unicode scalar values (`MAX_SEARCH_TEXT`). */
const MAX_SEARCH_TEXT = 256;
const U32_MAX = 0xffff_ffffn;
const U64_MAX = (1n << 64n) - 1n;
const I128_MIN = -(1n << 127n);
const I128_MAX = (1n << 127n) - 1n;

const engineError = (code: "invalid_query" | "stale_cursor", message: string): AppError => ({ code, message, data: null });
const staleCursor = () => engineError("stale_cursor", "history cursor does not match the query");

/** Rust's `char::is_whitespace` (Unicode White_Space): what `str::trim` strips. Not JS `trim`,
 * which also strips U+FEFF and keeps U+0085. */
const RUST_WS = "\\t-\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const RUST_TRIM = new RegExp(`^[${RUST_WS}]+|[${RUST_WS}]+$`, "gu");
const rustTrim = (s: string) => s.replace(RUST_TRIM, "");

/** Characters Rust's `str` Debug writes as `\u{..}`: grapheme extenders and what
 * `char::is_printable` refuses. Checked against rustc 1.98 for every code point; a Unicode
 * version difference between Rust and the JS engine would only change the fixture's own tag. */
const DEBUG_UNICODE_ESCAPE = /[\p{Grapheme_Extend}\p{Cc}\p{Cf}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}\u115f\u1160\u3164\uffa0]/u;
const DEBUG_SHORT: Record<string, string> = { "\0": "\\0", "\t": "\\t", "\r": "\\r", "\n": "\\n", '"': '\\"', "\\": "\\\\" };

/** `format!("{s:?}")` of a Rust `&str`. */
function rustDebugStr(s: string): string {
  let out = '"';
  for (const ch of s) {
    const cp = ch.codePointAt(0)!;
    if (ch in DEBUG_SHORT) out += DEBUG_SHORT[ch];
    else if (cp !== 0x20 && cp !== 0xff9e && cp !== 0xff9f && DEBUG_UNICODE_ESCAPE.test(ch)) out += `\\u{${cp.toString(16)}}`;
    else out += ch;
  }
  return out + '"';
}

/** A serde camelCase variant name back to the Rust variant. */
const variant = (name: string) => name[0].toUpperCase() + name.slice(1);
const debugOpt = (v: number | null) => (v === null ? "None" : `Some(${v})`);

/** The engine's `HistoryFilter` as `HistoryRequest::into_query` builds it. */
interface Filter {
  types: TxTypeName[];
  categories: TxCategoryName[];
  statuses: TxStatusKindName[];
  dateFrom: number | null;
  dateTo: number | null;
  /** Untrimmed; `null` when blank. */
  text: string | null;
  minAmount: number | null;
  watchOnly: WatchOnlyFilterName;
}

function filterOf(r: HistoryRequest): Filter {
  return {
    types: r.types ?? [],
    categories: r.categories,
    statuses: r.statuses ?? [],
    dateFrom: r.dateFrom ?? null,
    dateTo: r.dateTo ?? null,
    text: r.text != null && rustTrim(r.text) !== "" ? r.text : null,
    minAmount: r.minAmount ?? null,
    watchOnly: r.watchOnly ?? "all",
  };
}

/** `query_tag`: FNV-1a 64 over `format!("{filter:?}|{sort:?}")`. */
function queryTag(f: Filter, sort: HistorySortName): bigint {
  const list = (xs: string[]) => `[${xs.map(variant).join(", ")}]`;
  const debug =
    `HistoryFilter { types: ${list(f.types)}, categories: ${list(f.categories)}, statuses: ${list(f.statuses)}, ` +
    `date_from: ${debugOpt(f.dateFrom)}, date_to: ${debugOpt(f.dateTo)}, ` +
    `text: ${f.text == null ? "None" : `Some(${rustDebugStr(f.text)})`}, min_amount: ${debugOpt(f.minAmount)}, ` +
    `watch_only: ${variant(f.watchOnly)} }|${variant(sort)}`;
  let h = 0xcbf2_9ce4_8422_2325n;
  for (const b of new TextEncoder().encode(debug)) h = BigInt.asUintN(64, (h ^ BigInt(b)) * 0x0100_0000_01b3n);
  return h;
}

/** `validate_query`: limit, then date range, then text length. */
function validate(limit: number, f: Filter) {
  if (limit === 0 || limit > MAX_PAGE) throw engineError("invalid_query", `limit ${limit} outside 1..=${MAX_PAGE}`);
  if (f.dateFrom != null && f.dateTo != null && f.dateFrom > f.dateTo)
    throw engineError("invalid_query", `date_from ${f.dateFrom} is after date_to ${f.dateTo}`);
  if (f.text != null && [...f.text].length > MAX_SEARCH_TEXT)
    throw engineError("invalid_query", `search text longer than ${MAX_SEARCH_TEXT} characters`);
}

/** `matches`; `needle` is the trimmed, lowercased text. */
function matches(r: TxRow, f: Filter, needle: string | null): boolean {
  if (f.types.length > 0 && !f.types.includes(r.txType)) return false;
  if (f.categories.length > 0 && !f.categories.includes(r.category)) return false;
  if (f.statuses.length > 0 && !f.statuses.includes(r.status.kind)) return false;
  if (f.dateFrom != null || f.dateTo != null) {
    const ts = r.timestamp;
    if (ts == null) return false;
    if ((f.dateFrom != null && ts < f.dateFrom) || (f.dateTo != null && ts >= f.dateTo)) return false;
  }
  if (f.minAmount != null && Math.abs(r.amount) < f.minAmount) return false;
  if (f.watchOnly === "yes" && !r.involvesWatchOnly) return false;
  if (f.watchOnly === "no" && r.involvesWatchOnly) return false;
  if (needle != null)
    return r.txid.includes(needle) || !!r.address?.toLowerCase().includes(needle) || !!r.label?.toLowerCase().includes(needle);
  return true;
}

/** Rust `str` order (UTF-8 bytes, i.e. code points), not JS UTF-16 code-unit order. */
function cmpStr(a: string, b: string): number {
  if (a === b) return 0;
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    let x = a.charCodeAt(i);
    let y = b.charCodeAt(i);
    if (x === y) continue;
    // Surrogates (astral code points) sort after U+E000..U+FFFF.
    if (x >= 0xd800 && y >= 0xd800) {
      x = x < 0xe000 ? x + 0x2000 : x - 0x800;
      y = y < 0xe000 ? y + 0x2000 : y - 0x800;
    }
    return x < y ? -1 : 1;
  }
  return a.length < b.length ? -1 : 1;
}

const byTime = (s: HistorySortName) => s === "newestFirst" || s === "oldestFirst";
const descending = (s: HistorySortName) => s === "newestFirst" || s === "amountDescending";

interface CursorKey {
  /** An i128 in the engine. */
  primary: bigint;
  txid: string;
  index: number;
}

/** `key_of`: the sort's primary (an unknown time is `u64::MAX`, the newest), txid, record index. */
const keyOf = (r: TxRow, s: HistorySortName): CursorKey => ({
  primary: byTime(s) ? (r.timestamp == null ? U64_MAX : BigInt(r.timestamp)) : BigInt(r.amount),
  txid: r.txid,
  index: r.recordIndex,
});

/** `compare`: the primary in the sort's direction, then txid, then record index. */
function cmpKey(a: CursorKey, b: CursorKey, s: HistorySortName): number {
  if (a.primary !== b.primary) return (a.primary < b.primary) !== descending(s) ? -1 : 1;
  return cmpStr(a.txid, b.txid) || Math.sign(a.index - b.index);
}

const encodeCursor = (tag: bigint, k: CursorKey) => `v1.${tag.toString(16).padStart(16, "0")}.${k.primary}.${k.txid}.${k.index}`;

/** `decode_cursor`, with Rust's integer parsing: an optional `+` (and `-` for the signed
 * primary), any number of digits, in range. */
function decodeCursor(cursor: string, tag: bigint): CursorKey {
  const parts = cursor.split(".");
  if (parts.length !== 5 || parts[0] !== "v1") throw staleCursor();
  const [, t, p, txid, i] = parts;
  const big = (re: RegExp, s: string, min: bigint, max: bigint, hex = false) => {
    if (!re.test(s)) throw staleCursor();
    const digits = s.replace(/^\+/, "");
    const v = hex ? BigInt(`0x${digits}`) : BigInt(digits);
    if (v < min || v > max) throw staleCursor();
    return v;
  };
  if (big(/^\+?[0-9a-fA-F]+$/, t, 0n, U64_MAX, true) !== tag) throw staleCursor();
  return {
    primary: big(/^[+-]?[0-9]+$/, p, I128_MIN, I128_MAX),
    txid,
    index: Number(big(/^\+?[0-9]+$/, i, 0n, U32_MAX)),
  };
}

/** `history_page` over `rows`: the engine's validation, filters, order and keyset cursors.
 * Throws the engine's `invalid_query` / `stale_cursor` AppErrors. */
export function historyPage(rows: TxRow[], request: HistoryRequest): HistoryRows {
  const { sort, cursor, limit } = request;
  const f = filterOf(request);
  validate(limit, f);
  const tag = queryTag(f, sort);
  const after = cursor != null ? decodeCursor(cursor, tag) : null;
  const needle = f.text == null ? null : rustTrim(f.text).toLowerCase();
  const hits = rows
    .filter((r) => matches(r, f, needle))
    .map((row) => ({ row, key: keyOf(row, sort) }))
    .sort((a, b) => cmpKey(a.key, b.key, sort));
  // `partition_point`: the first row after the cursor's key.
  let start = 0;
  if (after) {
    let hi = hits.length;
    while (start < hi) {
      const mid = (start + hi) >>> 1;
      if (cmpKey(hits[mid].key, after, sort) > 0) hi = mid;
      else start = mid + 1;
    }
  }
  const page = hits.slice(start, start + limit);
  const more = hits.length - start > limit;
  return {
    rows: page.map((h) => h.row),
    nextCursor: more ? encodeCursor(tag, page[page.length - 1].key) : null,
    totalMatching: hits.length,
  };
}

/** `tx_detail` of `row`: inputs, outputs and a raw size drawn from its txid. */
export function txDetails(row: TxRow): TxDetails {
  const txid = row.txid;
  const rand = prng(parseInt(txid.slice(0, 8), 16));
  const { hex, address } = makeIds(rand);
  const incoming = row.amount >= 0;
  const fee = row.fee ?? 0;
  const value = Math.abs(row.amount);
  const change = incoming ? 0 : Math.floor(rand() * COIN);
  const details = {
    txid,
    rows: [row],
    status: row.status,
    timestamp: row.timestamp,
    blockHeight: row.blockHeight,
    blockHash: row.blockHeight ? "000000" + hex(58) : null,
    fee: row.fee,
    sizeBytes: 192 + Math.floor(rand() * 200),
    inputs: [
      {
        previousTxid: hex(64),
        previousVout: Math.floor(rand() * 3),
        address: incoming ? address() : row.address,
        // A send row's amount already includes the fee: inputs = outputs + fee.
        amount: incoming ? null : value + change,
        isMine: !incoming,
      },
    ],
    outputs: [
      { vout: 0, address: row.address, amount: incoming ? value : value - fee, isMine: incoming, isChange: false, dataHex: null },
      ...(change > 0 ? [{ vout: 1, address: address(), amount: change, isMine: true, isChange: true, dataHex: null }] : []),
    ],
    message: null,
    label: row.label,
  };
  // Placeholder bytes of the stated size.
  return { ...details, rawHex: "00".repeat(details.sizeBytes) };
}
