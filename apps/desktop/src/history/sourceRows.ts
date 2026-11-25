// Activity rows of the other balances: Platform-address activity (lane E2's `platform_activity`) and
// the shielded account's (lane E1's `shielded_history`). The engine's history knows Core
// transactions only, so these are synthetic `TxRow`s, told apart by their `platform:` or
// `shielded:` txid (`<source>:<kind>:<id>`), shown in List mode among the Core rows by date with a
// source tag. They open their own details sheet, built from the activity record (they have no Core
// transaction), and CSV export stays the Core history.

import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { ShieldedActivity } from "../api/generated/ShieldedActivity";
import { creditsToDuffs } from "../api/pending";

export type RowSource = "platform" | "shielded";

export const SOURCE_TAG: Record<RowSource, string> = { platform: "Platform", shielded: "Shielded" };

export const SourceRowText = {
  platformIncoming: "Received on Platform",
  platformOutgoing: "Sent from Platform",
  moveToCore: "Moved to Dash wallet",
  platformSend: "Sent to a Platform address",
  /** A top-up of the wallet's own identity paid from the Platform addresses. */
  toIdentity: "Credit top-up from Platform",
  /** "Add from Dash wallet": the lock of the Core coins and what Platform added for it. */
  fromCore: "Moved from Dash wallet",
  /** An unshield to this wallet's own Platform address, seen from the address (C-routes D-C1). */
  fromShielded: "From Shielded",
  /** A move the engine superseded with a retry: neither a loss nor a completed move. */
  replaced: "Replaced by a retry",
  /** A move, send or top-up that failed by height but may still execute: not "Not accepted". */
  mayLand: "May still go through",
  shieldedAdd: "Added to Shielded",
  shieldedToPlatform: "Moved to Platform",
  shieldedSentToPlatform: "Sent to Platform",
  shieldedReceived: "Received in Shielded",
  shieldedSent: "Sent from Shielded",
  shieldedPaid: "Paid from Shielded",
  /** A payment to a DashPay contact from the Platform balance (C-funding P10). */
  contactPayment: "Paid from Platform",
};

const REPLACED = ":replaced";
const MAY_LAND = ":mayland";

/** A Platform move the engine replaced with a retry: it carries no amount and is never a loss. */
export const isReplacedRow = (row: Pick<TxRow, "txid">): boolean => row.txid.endsWith(REPLACED);

/** A failed Platform move or send, or a Shielded spend Platform refused whose funds stay held, that
 * may still execute: shown as unconfirmed, never as a problem. */
export const isMayLandRow = (row: Pick<TxRow, "txid">): boolean => row.txid.endsWith(MAY_LAND);

/** The source of a synthetic row; `null` for a Core transaction. */
export function rowSource(row: Pick<TxRow, "txid">): RowSource | null {
  return row.txid.startsWith("platform:") ? "platform" : row.txid.startsWith("shielded:") ? "shielded" : null;
}

const statusOf = (kind: TxRow["status"]["kind"]): TxRow["status"] => ({ kind, confirmations: kind === "confirmed" ? 1 : 0, instantLocked: false, chainLocked: false, maturesIn: null });

const base = (txid: string, status: TxRow["status"], timestamp: number | null, amount: number): TxRow => ({
  txid,
  recordIndex: 0,
  txType: "other",
  category: "other",
  status,
  timestamp,
  blockHeight: null,
  amount,
  fee: null,
  address: null,
  label: null,
  countsTowardBalance: false,
  involvesWatchOnly: false,
});

/** A Platform-address activity row as a history row. A move shows what arrives in the Dash wallet
 * (an internal move: no sign); a send pays its recipient (`to`); a failed one is the "not accepted"
 * problem row, unless it may still execute (`mayLand`): that one is unconfirmed, with its amount. */
export function platformRow(a: PlatformActivity): TxRow {
  const move = a.kind === "moveToCore";
  const replaced = a.state === "replaced";
  // What a fund or an unshield adds to the Platform addresses reads as a receipt there.
  const received = a.kind === "incoming" || a.kind === "fromCore" || a.kind === "fromShielded";
  // A move with no proved arrival shows what left (`credits`), never an amount that "arrives".
  const amount = replaced ? 0 : move ? (a.expectedDuffs ?? creditsToDuffs(a.credits)) : creditsToDuffs(a.credits);
  const topUp = a.kind === "toIdentity";
  const paid = a.kind === "outgoing" || a.kind === "send" || a.kind === "contactPayment" || topUp;
  const mayLand = a.state === "failed" && a.mayLand && (move || a.kind === "send" || a.kind === "contactPayment" || topUp);
  const kind = a.state === "done" || replaced ? "confirmed" : a.state === "pending" || mayLand ? "unconfirmed" : "notAccepted";
  return {
    ...base(`platform:${a.kind}:${a.id}${replaced ? REPLACED : mayLand ? MAY_LAND : ""}`, statusOf(kind), a.at, paid && amount !== 0 ? -amount : amount),
    txType: move ? "platformTransfer" : received ? "recvWithAddress" : "sendToAddress",
    category: move ? "platform" : received ? "received" : "sent",
    address: move || a.kind === "send" || a.kind === "contactPayment" ? a.to : a.address,
    ...(a.contact ? { counterparty: a.contact } : {}),
  };
}

/** A shielded activity row as a history row. Adding to the pool and a withdrawal to the Dash wallet
 * are internal moves (no sign); an `unshield` pays a Platform address (E1's contract), so it is
 * a payment out unless the engine says it is a self-transfer; the rest are received or paid. A
 * spend Platform refused whose funds stay held (`refusal: held`) may still go through: unconfirmed,
 * with its amount, never "Not accepted". */
export function shieldedRow(a: ShieldedActivity): TxRow {
  const internal = a.kind === "shield" || a.kind === "shieldFromCore" || a.kind === "withdrawal" || a.direction === "selfTransfer";
  const amount = creditsToDuffs(a.credits);
  const received = a.direction === "in";
  const held = a.refusal === "held";
  const kind = a.status === "confirmed" ? "confirmed" : a.status === "failed" && !held ? "notAccepted" : "unconfirmed";
  return {
    ...base(`shielded:${a.kind}:${a.id}${held ? MAY_LAND : ""}`, statusOf(kind), a.createdAt, a.direction === "out" ? -amount : amount),
    blockHeight: a.blockHeight,
    txType: internal ? "platformTransfer" : received ? "recvWithAddress" : "sendToAddress",
    category: internal ? "platform" : received ? "received" : "sent",
    address: a.counterparty,
    label: a.memo,
  };
}

export type SourceRecord =
  | { source: "platform"; row: TxRow; record: PlatformActivity }
  | { source: "shielded"; row: TxRow; record: ShieldedActivity };

/** A row's txid without its state suffix: a move that is replaced or may still land while its
 * details are open is still the same record. */
const recordKey = (txid: string) => txid.replace(/:(replaced|mayland)$/, "");

/** The activity record a synthetic row was made from, by its txid; `null` for a Core txid or a
 * record no longer in the read. */
export function findSourceRecord(txid: string, platform: readonly PlatformActivity[] | null | undefined, shielded: readonly ShieldedActivity[] | null | undefined): SourceRecord | null {
  const key = recordKey(txid);
  const source = rowSource({ txid });
  if (source === "platform") {
    const record = platform?.find((a) => recordKey(platformRow(a).txid) === key);
    return record ? { source, row: platformRow(record), record } : null;
  }
  if (source === "shielded") {
    const record = shielded?.find((a) => recordKey(shieldedRow(a).txid) === key);
    return record ? { source, row: shieldedRow(record), record } : null;
  }
  return null;
}

export function sourceRowTitle(row: TxRow): string {
  const kind = row.txid.split(":")[1];
  if (rowSource(row) === "platform")
    return kind === "moveToCore" ? SourceRowText.moveToCore : kind === "contactPayment" ? SourceRowText.contactPayment : kind === "send" ? SourceRowText.platformSend : kind === "toIdentity" ? SourceRowText.toIdentity : kind === "fromCore" ? SourceRowText.fromCore : kind === "fromShielded" ? SourceRowText.fromShielded : kind === "incoming" ? SourceRowText.platformIncoming : SourceRowText.platformOutgoing;
  switch (kind) {
    case "shield":
    case "shieldFromCore":
      return SourceRowText.shieldedAdd;
    case "unshield":
      return row.category === "platform" ? SourceRowText.shieldedToPlatform : SourceRowText.shieldedSentToPlatform;
    case "withdrawal":
      return SourceRowText.moveToCore;
    case "received":
      return SourceRowText.shieldedReceived;
    case "sent":
      return SourceRowText.shieldedSent;
    default:
      return SourceRowText.shieldedPaid;
  }
}

export interface SourceFilter {
  dateFrom?: number;
  /** Exclusive. */
  dateTo?: number;
  minAmount?: number;
}

/** The `extra` rows the date range and minimum amount keep, as the engine applies them to Core
 * rows (a row without a time is the newest: a date range drops it, as it does Core's). */
export function filterExtra(extra: TxRow[], filter: SourceFilter): TxRow[] {
  const dated = filter.dateFrom !== undefined || filter.dateTo !== undefined;
  return extra.filter(
    (r) =>
      (r.timestamp === null ? !dated : (filter.dateFrom === undefined || r.timestamp >= filter.dateFrom) && (filter.dateTo === undefined || r.timestamp < filter.dateTo)) &&
      (filter.minAmount === undefined || Math.abs(r.amount) >= filter.minAmount),
  );
}

/** Newest first; a row without a time (a fresh arrival) is the newest, as the engine sorts it. */
const at = (r: TxRow) => r.timestamp ?? Number.POSITIVE_INFINITY;
const newestFirst = (a: TxRow, b: TxRow) => (at(a) === at(b) ? 0 : at(b) > at(a) ? 1 : -1);

/** Core's `rows` (newest first, `more`: further pages exist) with the other sources' `extra` rows
 * (already filtered: `filterExtra`) among them by date. While Core has further pages, extra rows
 * older than the oldest loaded Core row wait for them, so a row never appears above where a later
 * page puts the Core rows. */
export function mergeActivity(rows: TxRow[], more: boolean, extra: TxRow[]): TxRow[] {
  const oldest = more ? (rows.at(-1)?.timestamp ?? null) : null;
  const shown = oldest === null ? extra : extra.filter((r) => at(r) >= oldest);
  // Stable: on a tie the Core row stays first.
  return shown.length === 0 ? rows : [...rows, ...shown].sort(newestFirst);
}
