// Platform-address activity (lane E2's `platform_activity`) as Activity rows. The engine's history
// knows Core transactions only, so these are synthetic `TxRow`s, told apart by their `platform:`
// txid, shown in List mode among the Core rows by date with a "Platform" tag. They open no detail
// (a Platform transfer has no Core transaction), and CSV export stays the Core history.

import type { TxRow } from "../api/generated/TxRow";
import type { PlatformActivity } from "../api/pending";

export const PLATFORM_TXID_PREFIX = "platform:";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), the ratio `PlatformAddressBalance.duffs`
 * uses. */
const CREDITS_PER_DUFF = 1000n;

export const PLATFORM_TAG = "Platform";

export const PlatformRowText = {
  incoming: "Received on Platform",
  outgoing: "Sent from Platform",
  moveToCore: "Moved to Dash wallet",
};

export const isPlatformRow = (row: Pick<TxRow, "txid">) => row.txid.startsWith(PLATFORM_TXID_PREFIX);

const duffsOf = (credits: string) => Number(BigInt(credits) / CREDITS_PER_DUFF);

/** One activity row as a history row. A move shows what arrives in the Dash wallet (an
 * internal move: no sign); a failed one is the "not accepted" problem row. */
export function platformRow(a: PlatformActivity): TxRow {
  const status: TxRow["status"] = {
    kind: a.state === "done" ? "confirmed" : a.state === "pending" ? "unconfirmed" : "notAccepted",
    confirmations: a.state === "done" ? 1 : 0,
    instantLocked: false,
    chainLocked: false,
    maturesIn: null,
  };
  const move = a.kind === "moveToCore";
  const amount = move ? (a.expectedDuffs ?? duffsOf(a.credits)) : duffsOf(a.credits);
  return {
    txid: `${PLATFORM_TXID_PREFIX}${a.id}`,
    recordIndex: 0,
    txType: move ? "platformTransfer" : a.kind === "incoming" ? "recvWithAddress" : "sendToAddress",
    category: move ? "platform" : a.kind === "incoming" ? "received" : "sent",
    status,
    timestamp: a.at,
    blockHeight: null,
    amount: a.kind === "outgoing" ? -amount : amount,
    fee: null,
    address: move ? a.to : a.address,
    label: null,
    countsTowardBalance: false,
    involvesWatchOnly: false,
  };
}

export const platformRowTitle = (row: TxRow): string =>
  row.txType === "platformTransfer" ? PlatformRowText.moveToCore : row.amount >= 0 ? PlatformRowText.incoming : PlatformRowText.outgoing;

export interface PlatformFilter {
  dateFrom?: number;
  /** Exclusive. */
  dateTo?: number;
  minAmount?: number;
}

/** Core's `rows` (newest first, `more`: further pages exist) with the Platform `activity` among
 * them by date. While Core has further pages, Platform rows older than the oldest loaded Core row
 * wait for them, so a row never appears above where a later page puts the Core rows. The date
 * range and minimum amount apply as the engine applies them to Core rows. */
export function mergeActivity(rows: TxRow[], more: boolean, activity: PlatformActivity[] | null | undefined, filter: PlatformFilter = {}): TxRow[] {
  if (!activity?.length) return rows;
  const oldest = more ? (rows.at(-1)?.timestamp ?? null) : null;
  const platform = activity
    .map(platformRow)
    .filter((r) => {
      const at = r.timestamp ?? 0;
      return (
        (filter.dateFrom === undefined || at >= filter.dateFrom) &&
        (filter.dateTo === undefined || at < filter.dateTo) &&
        (filter.minAmount === undefined || Math.abs(r.amount) >= filter.minAmount) &&
        (oldest === null || at >= oldest)
      );
    });
  if (platform.length === 0) return rows;
  // Stable: on a tie the Core row stays first.
  return [...rows, ...platform].sort((a, b) => (b.timestamp ?? 0) - (a.timestamp ?? 0));
}
