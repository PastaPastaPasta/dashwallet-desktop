// Activity rows of the other balances: Platform-address activity (lane E2's `platform_activity`) and
// the shielded account's (lane E1's `shielded_history`). The engine's history knows Core
// transactions only, so these are synthetic `TxRow`s, told apart by their `platform:` or
// `shielded:` txid (`<source>:<kind>:<id>`), shown in List mode among the Core rows by date with a
// source tag. They open their own details sheet, built from the activity record (they have no Core
// transaction), and CSV export stays the Core history.

import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { ShieldedActivity } from "../api/generated/ShieldedActivity";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { creditsToDuffs } from "../api/pending";
import { t } from "../i18n";

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
  /** The value left on a transfer to Shielded that Platform refused, recovered to the Platform balance. */
  recovered: "Recovered from an unfinished transfer",
  /** What Platform returned to this wallet's Platform address when it could not create the
   * identity a username paid from Shielded (C-funding F12). */
  registrationRefund: "Username payment refund",
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
  /** The cancel of a held move to the Dash wallet: a send to the wallet's own Platform address. */
  cancelMove: "Cancel of an earlier move",
  /** The cancel of a held contact payment from Platform. */
  cancelPayment: "Cancel of an earlier payment",
};

const REPLACED = ":replaced";
const MAY_LAND = ":mayland";
const SETTLED_UNKNOWN = ":unknown";

/** A Platform move the engine replaced with a retry: it carries no amount and is never a loss. */
export const isReplacedRow = (row: Pick<TxRow, "txid">): boolean => row.txid.endsWith(REPLACED);

/** A failed Platform move or send, or a Shielded spend Platform refused whose funds stay held, that
 * may still execute: shown as unconfirmed, never as a problem. */
export const isMayLandRow = (row: Pick<TxRow, "txid">): boolean => row.txid.endsWith(MAY_LAND);

/** A row of a group that settled with no evidence of which member went through: only the newest
 * (done) carries the group's debit, so Activity never counts it twice. */
export const isSettledUnknownRow = (row: Pick<TxRow, "txid">): boolean => row.txid.endsWith(SETTLED_UNKNOWN);

const SETTLED_KEYS = {
  transfer: { title: "transfer.status.settledUnknown.title", text: "transfer.status.settledUnknown.text" },
  pay: { title: "pay.status.settledUnknown.title", text: "pay.status.settledUnknown.text" },
  topUp: { title: "dashpay.topUp.status.settledUnknown.title", text: "dashpay.topUp.status.settledUnknown.text" },
} as const;

/** A settled-unknown row's title and text, by its kind: a payment, a top-up, else a move. */
export function settledUnknownText(row: Pick<TxRow, "txid">): { title: string; text: string } {
  const kind = row.txid.split(":")[1];
  const keys = kind === "send" || kind === "contactPayment" ? SETTLED_KEYS.pay : kind === "toIdentity" ? SETTLED_KEYS.topUp : SETTLED_KEYS.transfer;
  return { title: t(keys.title), text: t(keys.text) };
}

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

/** The kind a row's txid carries: a cancel's names what it cancels (`all`: the activity it is in;
 * a cancel of a row no longer listed reads as a move's). */
function rowKind(a: PlatformActivity, all: readonly PlatformActivity[] | null | undefined): string {
  if (!a.cancels) return a.kind;
  return all?.find((b) => b.id === a.cancels)?.kind === "contactPayment" ? "cancelPayment" : "cancelMove";
}

/** A Platform-address activity row as a history row. A move shows what arrives in the Dash wallet
 * (an internal move: no sign); a send pays its recipient (`to`); a failed one is the "not accepted"
 * problem row, unless it may still execute (`mayLand`): that one is unconfirmed, with its amount. A
 * cancel is a send to the wallet's own address: its amount out, what arrives an incoming row. `all`:
 * the activity the row is in, which names what a cancel cancels. */
export function platformRow(a: PlatformActivity, all?: readonly PlatformActivity[] | null): TxRow {
  const move = a.kind === "moveToCore";
  const unknown = a.winnerKnown === false;
  const replaced = a.state === "replaced" && !unknown;
  // What a fund, an unshield, a recover or a registration refund adds to the Platform addresses reads as a receipt there.
  const received = a.kind === "incoming" || a.kind === "fromCore" || a.kind === "fromShielded" || a.kind === "recovered" || a.kind === "registrationRefund";
  // A move with no proved arrival shows what left (`credits`), never an amount that "arrives".
  const amount = replaced || (unknown && a.state !== "done") ? 0 : move ? (a.expectedDuffs ?? creditsToDuffs(a.credits)) : creditsToDuffs(a.credits);
  const topUp = a.kind === "toIdentity";
  const paid = a.kind === "outgoing" || a.kind === "send" || a.kind === "contactPayment" || topUp;
  const mayLand = !unknown && a.state === "failed" && a.mayLand && (move || a.kind === "send" || a.kind === "contactPayment" || topUp);
  const kind = a.state === "done" || replaced || unknown ? "confirmed" : a.state === "pending" || mayLand ? "unconfirmed" : "notAccepted";
  return {
    ...base(`platform:${rowKind(a, all)}:${a.id}${unknown ? SETTLED_UNKNOWN : replaced ? REPLACED : mayLand ? MAY_LAND : ""}`, statusOf(kind), a.at, paid && amount !== 0 ? -amount : amount),
    txType: move ? "platformTransfer" : received ? "recvWithAddress" : "sendToAddress",
    category: move ? "platform" : received ? "received" : "sent",
    address: move || a.kind === "send" || a.kind === "contactPayment" ? a.to : a.address,
    ...(a.contact ? { counterparty: a.contact } : {}),
  };
}

/** A shielded activity row as a history row. Adding to the pool and a withdrawal to the Dash wallet
 * are internal moves (no sign); an `unshield` pays a Platform address (E1's contract), so it is
 * a payment out unless the engine says it is a self-transfer; the rest are received or paid. A
 * spend the engine holds open (`refusal: held`: refused, or its outcome not known yet), or with no refusal whose
 * notes are held (`held`, the status's `heldSpends`), may still go through: unconfirmed, with its
 * amount, never "Not accepted". A released refusal reads as released whatever `held` says. */
export function shieldedRow(a: ShieldedActivity, held?: ReadonlySet<string>): TxRow {
  const internal = a.kind === "shield" || a.kind === "shieldFromCore" || a.kind === "withdrawal" || a.direction === "selfTransfer";
  const amount = creditsToDuffs(a.credits);
  const received = a.direction === "in";
  // The engine's refusal reads the library's holds with the journal, and a release is final: it
  // outranks the status's `heldSpends`, which may be from an earlier read.
  const mayLand = a.status !== "confirmed" && (a.refusal ? a.refusal === "held" : !!held?.has(a.id));
  const kind = a.status === "confirmed" ? "confirmed" : a.status === "failed" && !mayLand ? "notAccepted" : "unconfirmed";
  return {
    ...base(`shielded:${a.kind}:${a.id}${mayLand ? MAY_LAND : ""}`, statusOf(kind), a.createdAt, a.direction === "out" ? -amount : amount),
    blockHeight: a.blockHeight,
    txType: internal ? "platformTransfer" : received ? "recvWithAddress" : "sendToAddress",
    category: internal ? "platform" : received ? "received" : "sent",
    address: a.counterparty,
    label: a.memo,
  };
}

/** The history ids of the Shielded spends whose notes are held. */
export const heldActivities = (status: ShieldedStatus | null | undefined): ReadonlySet<string> => new Set(status?.heldSpends.map((h) => h.activityId));

export type SourceRecord =
  | { source: "platform"; row: TxRow; record: PlatformActivity }
  | { source: "shielded"; row: TxRow; record: ShieldedActivity };

/** A row's txid without its state suffix: a move that is replaced or may still land while its
 * details are open is still the same record. */
const recordKey = (txid: string) => txid.replace(/:(replaced|mayland|unknown)$/, "");

/** The activity record a synthetic row was made from, by its txid; `null` for a Core txid or a
 * record no longer in the read. `held`: the Shielded spends whose notes are held. */
export function findSourceRecord(txid: string, platform: readonly PlatformActivity[] | null | undefined, shielded: readonly ShieldedActivity[] | null | undefined, held?: ReadonlySet<string>): SourceRecord | null {
  const key = recordKey(txid);
  const source = rowSource({ txid });
  if (source === "platform") {
    const record = platform?.find((a) => recordKey(platformRow(a, platform).txid) === key);
    return record ? { source, row: platformRow(record, platform), record } : null;
  }
  if (source === "shielded") {
    const record = shielded?.find((a) => recordKey(shieldedRow(a).txid) === key);
    return record ? { source, row: shieldedRow(record, held), record } : null;
  }
  return null;
}

export function sourceRowTitle(row: TxRow): string {
  const kind = row.txid.split(":")[1];
  if (rowSource(row) === "platform")
    return isSettledUnknownRow(row) ? settledUnknownText(row).title : kind === "moveToCore" ? SourceRowText.moveToCore : kind === "contactPayment" ? SourceRowText.contactPayment : kind === "cancelMove" ? SourceRowText.cancelMove : kind === "cancelPayment" ? SourceRowText.cancelPayment : kind === "send" ? SourceRowText.platformSend : kind === "toIdentity" ? SourceRowText.toIdentity : kind === "fromCore" ? SourceRowText.fromCore : kind === "recovered" ? SourceRowText.recovered : kind === "fromShielded" ? SourceRowText.fromShielded : kind === "registrationRefund" ? SourceRowText.registrationRefund : kind === "incoming" ? SourceRowText.platformIncoming : SourceRowText.platformOutgoing;
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
