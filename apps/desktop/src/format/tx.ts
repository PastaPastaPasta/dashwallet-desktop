// List-mode transaction presentation (UX-SPEC §4.9, §5.4, §5.5, §5.7), ported from
// Sources/WalletFeatures/Common/Presentation.swift so both stacks title, chip and group
// transactions the same way.

import type { TxCategoryName } from "../api/generated/TxCategoryName";
import type { TxRow } from "../api/generated/TxRow";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import type { TxTypeName } from "../api/generated/TxTypeName";

export const TxTitle = {
  received: "Received",
  sent: "Sent",
  sentToYourself: "Sent to yourself",
  internalTransfer: "Internal transfer",
  mixing: "Mixing",
  coinJoinSend: "CoinJoin Send",
  masternodeReward: "Masternode reward",
  mined: "Mined",
  providerTransaction: "Provider transaction (ProTx)",
  assetLock: "Asset lock",
  chipPending: "Pending",
  chipInstantSend: "InstantSend",
  chipConflicted: "Conflicted",
  chipAbandoned: "Abandoned",
  chipNotAccepted: "Not accepted",
  locked: "Locked",
  today: "Today",
  yesterday: "Yesterday",
} as const;

const MIXING: TxTypeName[] = [
  "recvWithCoinJoin",
  "coinJoinMixing",
  "coinJoinCollateralPayment",
  "coinJoinMakeCollaterals",
  "coinJoinCreateDenominations",
];

export function typeTitle(type: TxTypeName, category: TxCategoryName | null, amount: number): string {
  if (category === "reward" && type !== "generated") return TxTitle.masternodeReward;
  switch (type) {
    case "recvWithAddress":
    case "recvFromOther":
    case "dustReceive":
      return TxTitle.received;
    case "sendToAddress":
    case "sendToOther":
      return TxTitle.sent;
    case "sendToSelf":
      return TxTitle.sentToYourself;
    case "coinJoinSend":
      return TxTitle.coinJoinSend;
    case "generated":
      return TxTitle.mined;
    case "masternodeRegistration":
    case "masternodeUpdate":
      return TxTitle.providerTransaction;
    case "assetLock":
      return TxTitle.assetLock;
    case "platformTransfer":
      return TxTitle.internalTransfer;
    case "dataTransaction":
    case "other":
      return amount >= 0 ? TxTitle.received : TxTitle.sent;
    default:
      return MIXING.includes(type) ? TxTitle.mixing : amount >= 0 ? TxTitle.received : TxTitle.sent;
  }
}

/** Label first, then the type title. Never the raw address. */
export function txTitle(row: Pick<TxRow, "txType" | "category" | "amount" | "label">): string {
  return row.label ? row.label : typeTitle(row.txType, row.category, row.amount);
}

export type TxIcon = "received" | "sent" | "internalTransfer" | "mixing" | "mining" | "error";

export function txIcon(type: TxTypeName, status: TxStatusInfo, amount: number): TxIcon {
  if (status.kind === "conflicted" || status.kind === "abandoned" || status.kind === "notAccepted") return "error";
  if (type === "generated") return "mining";
  if (type === "sendToSelf" || type === "platformTransfer") return "internalTransfer";
  if (MIXING.includes(type)) return "mixing";
  return amount >= 0 ? "received" : "sent";
}

/** One status chip, or null when the transaction is settled ("quiet when fine"). */
export function txChip(status: TxStatusInfo): string | null {
  switch (status.kind) {
    case "conflicted":
      return TxTitle.chipConflicted;
    case "abandoned":
      return TxTitle.chipAbandoned;
    case "notAccepted":
      return TxTitle.chipNotAccepted;
    case "immature":
    case "confirmed":
      return null;
    case "unconfirmed":
      return status.instantLocked ? TxTitle.chipInstantSend : TxTitle.chipPending;
    case "confirming":
      if (status.chainLocked) return null;
      return status.instantLocked ? TxTitle.chipInstantSend : null;
  }
}

export function chipIsProblem(status: TxStatusInfo): boolean {
  return status.kind === "conflicted" || status.kind === "abandoned" || status.kind === "notAccepted";
}

export function trailingStatus(status: TxStatusInfo): string | null {
  return status.kind === "immature" ? TxTitle.locked : null;
}

/** A payment to yourself or an internal move: no sign on the amount. */
export function isInternal(type: TxTypeName): boolean {
  return type === "sendToSelf" || type === "platformTransfer";
}

/** First and last twelve characters around an ellipsis when longer than 24 characters. */
export function shortAddress(address: string): string {
  return address.length > 24 ? `${address.slice(0, 12)}…${address.slice(-12)}` : address;
}

// ---------------------------------------------------------------- dates

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());

export function dayTitle(day: Date, now: Date, locale?: string): string {
  const today = startOfDay(now).getTime();
  const that = startOfDay(day).getTime();
  if (that === today) return TxTitle.today;
  const yesterday = startOfDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)).getTime();
  if (that === yesterday) return TxTitle.yesterday;
  return day.toLocaleDateString(locale, { year: "numeric", month: "long", day: "numeric" });
}

export const weekday = (day: Date, locale?: string) => day.toLocaleDateString(locale, { weekday: "long" });

export const timeOfDay = (d: Date, locale?: string) => d.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });

/** "Oct 6, 2026 at 1:39 AM" (UX-SPEC §5.7 detail style). */
export function detailDate(d: Date, locale?: string): string {
  const date = d.toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
  return `${date} at ${timeOfDay(d, locale)}`;
}

export interface DayBucket<T> {
  /** Local midnight, or null for undated items (they go last). */
  day: Date | null;
  items: T[];
}

/** Groups by calendar day, newest day first, keeping the order inside a day. */
export function groupByDay<T>(items: T[], date: (item: T) => Date | null): DayBucket<T>[] {
  const order: (number | null)[] = [];
  const byDay = new Map<number | null, T[]>();
  for (const item of items) {
    const d = date(item);
    const key = d ? startOfDay(d).getTime() : null;
    if (!byDay.has(key)) {
      order.push(key);
      byDay.set(key, []);
    }
    byDay.get(key)!.push(item);
  }
  order.sort((a, b) => (a === null ? 1 : b === null ? -1 : b - a));
  return order.map((k) => ({ day: k === null ? null : new Date(k), items: byDay.get(k)! }));
}

export const rowDate = (row: Pick<TxRow, "timestamp">) => (row.timestamp ? new Date(row.timestamp * 1000) : null);
