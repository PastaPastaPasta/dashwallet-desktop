// Activity rows of the other balances: Platform-address activity (lane E2's `platform_activity`) and
// the shielded account's (lane E1's `shielded_history`). The engine's history knows Core
// transactions only, so these are synthetic `TxRow`s, told apart by their `platform:` or
// `shielded:` txid (`<source>:<kind>:<id>`), shown in List mode among the Core rows by date with a
// source tag. They open no detail (they have no Core transaction), and CSV export stays the Core
// history.

import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity, ShieldedActivity } from "../api/pending";

export type RowSource = "platform" | "shielded";

export const SOURCE_TAG: Record<RowSource, string> = { platform: "Platform", shielded: "Shielded" };

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), the ratio `PlatformAddressBalance.duffs`
 * uses. */
const CREDITS_PER_DUFF = 1000n;

export const SourceRowText = {
  platformIncoming: "Received on Platform",
  platformOutgoing: "Sent from Platform",
  moveToCore: "Moved to Dash wallet",
  shieldedAdd: "Added to Shielded",
  shieldedReceived: "Received in Shielded",
  shieldedSent: "Sent from Shielded",
  shieldedPaid: "Paid from Shielded",
};

/** The source of a synthetic row; `null` for a Core transaction. */
export function rowSource(row: Pick<TxRow, "txid">): RowSource | null {
  return row.txid.startsWith("platform:") ? "platform" : row.txid.startsWith("shielded:") ? "shielded" : null;
}

export const isSourceRow = (row: Pick<TxRow, "txid">) => rowSource(row) !== null;

const duffsOf = (credits: string) => Number(BigInt(credits) / CREDITS_PER_DUFF);

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
 * (an internal move: no sign); a failed one is the "not accepted" problem row. */
export function platformRow(a: PlatformActivity): TxRow {
  const move = a.kind === "moveToCore";
  const amount = move ? (a.expectedDuffs ?? duffsOf(a.credits)) : duffsOf(a.credits);
  return {
    ...base(`platform:${a.kind}:${a.id}`, statusOf(a.state === "done" ? "confirmed" : a.state === "pending" ? "unconfirmed" : "notAccepted"), a.at, a.kind === "outgoing" ? -amount : amount),
    txType: move ? "platformTransfer" : a.kind === "incoming" ? "recvWithAddress" : "sendToAddress",
    category: move ? "platform" : a.kind === "incoming" ? "received" : "sent",
    address: move ? a.to : a.address,
  };
}

/** A shielded activity row as a history row. Moving funds in or out of the pool is an internal
 * move (no sign); the rest are received or paid. */
export function shieldedRow(a: ShieldedActivity): TxRow {
  const internal = a.kind === "shield" || a.kind === "shieldFromCore" || a.kind === "unshield" || a.kind === "withdrawal";
  const amount = duffsOf(a.credits);
  const received = a.direction === "in";
  return {
    ...base(`shielded:${a.kind}:${a.id}`, statusOf(a.status === "confirmed" ? "confirmed" : a.status === "failed" ? "notAccepted" : "unconfirmed"), a.createdAt, a.direction === "out" ? -amount : amount),
    blockHeight: a.blockHeight,
    txType: internal ? "platformTransfer" : received ? "recvWithAddress" : "sendToAddress",
    category: internal ? "platform" : received ? "received" : "sent",
    address: a.counterparty,
    label: a.memo,
  };
}

export function sourceRowTitle(row: TxRow): string {
  const [source, kind] = row.txid.split(":");
  if (source === "platform") return kind === "moveToCore" ? SourceRowText.moveToCore : kind === "incoming" ? SourceRowText.platformIncoming : SourceRowText.platformOutgoing;
  switch (kind) {
    case "shield":
    case "shieldFromCore":
      return SourceRowText.shieldedAdd;
    case "unshield":
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

/** Core's `rows` (newest first, `more`: further pages exist) with the other sources' `extra` rows
 * among them by date. While Core has further pages, extra rows older than the oldest loaded Core
 * row wait for them, so a row never appears above where a later page puts the Core rows. The date
 * range and minimum amount apply as the engine applies them to Core rows. */
export function mergeActivity(rows: TxRow[], more: boolean, extra: TxRow[], filter: SourceFilter = {}): TxRow[] {
  if (extra.length === 0) return rows;
  const oldest = more ? (rows.at(-1)?.timestamp ?? null) : null;
  const shown = extra.filter((r) => {
    const at = r.timestamp ?? 0;
    return (
      (filter.dateFrom === undefined || at >= filter.dateFrom) &&
      (filter.dateTo === undefined || at < filter.dateTo) &&
      (filter.minAmount === undefined || Math.abs(r.amount) >= filter.minAmount) &&
      (oldest === null || at >= oldest)
    );
  });
  if (shown.length === 0) return rows;
  // Stable: on a tie the Core row stays first.
  return [...rows, ...shown].sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
}
