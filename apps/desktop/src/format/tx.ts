// List-mode transaction presentation (UX-SPEC §4.9, §5.4, §5.5, §5.7), ported from
// Sources/WalletFeatures/Common/Presentation.swift so both stacks title, chip and group
// transactions the same way.

import type { TxCategoryName } from "../api/generated/TxCategoryName";
import type { TxCounterparty } from "../api/generated/TxCounterparty";
import type { TxPurpose } from "../api/generated/TxPurpose";
import type { TxRow } from "../api/generated/TxRow";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import type { TxTypeName } from "../api/generated/TxTypeName";
import { initials } from "../contacts/model";
import { t } from "../i18n";

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
  assetLock: "Dash Platform funding",
  chipPending: "Pending",
  chipInstantSend: "InstantSend",
  chipConflicted: "Conflicted",
  chipAbandoned: "Abandoned",
  chipNotAccepted: "Not accepted",
  locked: "Locked",
  today: "Today",
  yesterday: "Yesterday",
} as const;

export type MixingGroup = { kind: "mixingGroup"; rows: TxRow[] };

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
    default:
      return MIXING.includes(type) ? TxTitle.mixing : amount >= 0 ? TxTitle.received : TxTitle.sent;
  }
}

/** What a Platform funding paid for, when the engine's registration records say. */
export function purposeTitle(purpose: TxPurpose | undefined): string | null {
  return purpose ? `Username @${purpose.username.replace(/^@/, "")}` : null;
}

/** How a DashPay counterparty (DP3-02) is named: its display name (trimmed; the contact chose
 * it), else `@username`; `null` while neither is known (the username lands later), never the
 * identity id. */
export function contactName(c: TxCounterparty | undefined): string | null {
  if (!c) return null;
  return c.displayName?.trim() || (c.username ? `@${c.username}` : null);
}

/** The initials avatar's letters for a counterparty (DP3-07 D1): those of the name `contactName`
 * shows; `null` while it has none, or a name with no letter, so the row keeps its type icon. */
export function contactLetters(c: TxCounterparty | undefined): string | null {
  const name = contactName(c);
  const letters = name === null ? null : initials(name);
  return letters === "?" ? null : letters;
}

/** Label first, then the DashPay contact's name (DP3-02), then the type title. Never the raw
 * address. A row's title comes only from here (U-03 D14). */
export function txTitle(row: Pick<TxRow, "txType" | "category" | "amount" | "label" | "counterparty" | "purpose">): string {
  return row.label || contactName(row.counterparty) || purposeTitle(row.purpose) || typeTitle(row.txType, row.category, row.amount);
}

/** Who a payment is with, in the activity row's words: the DashPay contact (`contactName`),
 * else the address label, else a shortened address. */
export function detailWho(row: Pick<TxRow, "label" | "address" | "counterparty">): string | null {
  const label = row.label?.trim() || null;
  return contactName(row.counterparty) ?? label ?? (row.address ? shortAddress(row.address) : null);
}

/** What happened, in the activity row's words: a username registration's purpose, else the type
 * title (`Sent`, `Received`, `Mixing`, …). */
export function detailWhat(row: Pick<TxRow, "txType" | "category" | "amount" | "purpose">): string {
  return purposeTitle(row.purpose) ?? typeTitle(row.txType, row.category, row.amount);
}

/** What a row says aloud about its contact when the title is something else, a label
 * (DP3-07 D13): the avatar is decoration, so "from Bob Builder" / "to Bob Builder"; `null` when the
 * title already names the contact or the row has no named contact. */
export function contactAside(row: Pick<TxRow, "amount" | "counterparty">, title: string): string | null {
  const name = contactName(row.counterparty);
  if (name === null || title === name) return null;
  return `${row.amount < 0 ? "to" : "from"} ${name}`;
}

/** dash-qt `TransactionTableModel::formatTxType`: the Type column of table mode. */
export function tableTypeTitle(type: TxTypeName): string {
  switch (type) {
    case "recvWithAddress":
      return "Received with";
    case "recvFromOther":
      return "Received from";
    case "recvWithCoinJoin":
      return "Received via CoinJoin";
    case "sendToAddress":
    case "sendToOther":
      return "Sent to";
    case "sendToSelf":
      return "Payment to yourself";
    case "generated":
      return "Mined";
    case "coinJoinMixing":
      return "CoinJoin Mixing";
    case "coinJoinCollateralPayment":
      return "CoinJoin Collateral Payment";
    case "coinJoinMakeCollaterals":
      return "CoinJoin Make Collateral Inputs";
    case "coinJoinCreateDenominations":
      return "CoinJoin Create Denominations";
    case "coinJoinSend":
      return "CoinJoin Send";
    case "platformTransfer":
      return "Platform Transfer";
    case "dustReceive":
      return "Dust Receive";
    case "dataTransaction":
      return "Data";
    case "masternodeRegistration":
      return "Masternode Registration";
    case "masternodeUpdate":
      return "Masternode Update";
    case "assetLock":
      return "Asset Lock";
    case "other":
      return "Other";
  }
}

/** Table mode's Address / Label column (dash-qt): the label, else the DashPay contact's name
 * (DP3-02, U-03 D14), else the address, else "(n/a)". */
export function rowParty(row: Pick<TxRow, "address" | "label" | "counterparty">): string {
  return row.label || contactName(row.counterparty) || row.address || "(n/a)";
}

// ---------------------------------------------------------------- filters

/** dash-qt's date filter, without Range (U-03 D12). */
export type DateChoice = "all" | "today" | "thisWeek" | "thisMonth" | "lastMonth" | "thisYear";

export const DATE_CHOICES: { id: DateChoice; title: string }[] = [
  { id: "all", title: "All" },
  { id: "today", title: "Today" },
  { id: "thisWeek", title: "This week" },
  { id: "thisMonth", title: "This month" },
  { id: "lastMonth", title: "Last month" },
  { id: "thisYear", title: "This year" },
];

/** The UNIX-second bounds of a date choice in local time (weeks start on Monday, as dash-qt's);
 * `dateTo` is exclusive, as the engine's `date_to`. */
export function dateRange(choice: DateChoice, now: Date): { dateFrom?: number; dateTo?: number } {
  const s = (d: Date) => Math.floor(d.getTime() / 1000);
  const y = now.getFullYear();
  const m = now.getMonth();
  switch (choice) {
    case "all":
      return {};
    case "today":
      return { dateFrom: s(startOfDay(now)) };
    case "thisWeek":
      return { dateFrom: s(new Date(y, m, now.getDate() - ((now.getDay() + 6) % 7))) };
    case "thisMonth":
      return { dateFrom: s(new Date(y, m, 1)) };
    case "lastMonth":
      return { dateFrom: s(new Date(y, m - 1, 1)), dateTo: s(new Date(y, m, 1)) };
    case "thisYear":
      return { dateFrom: s(new Date(y, 0, 1)) };
  }
}

export type TxIcon = "received" | "sent" | "internalTransfer" | "mixing" | "mining" | "error";

export function txIcon(type: TxTypeName, status: TxStatusInfo, amount: number): TxIcon {
  if (isProblem(status)) return "error";
  if (type === "generated") return "mining";
  if (isInternal(type)) return "internalTransfer";
  if (MIXING.includes(type)) return "mixing";
  return amount >= 0 ? "received" : "sent";
}

/** One status chip, or null when the transaction is settled ("quiet when fine"). */
export function txChip(status: TxStatusInfo): string | null {
  if (status.dropped) return t("activity.status.droppedLabel");
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
      return status.instantLocked && !status.chainLocked ? TxTitle.chipInstantSend : null;
  }
}

/** A collapsed mixing group's chip: Pending if any row is, else InstantSend if any row is. */
export function groupChip(rows: TxRow[]): string | null {
  const chips = rows.map((row) => txChip(row.status));
  return chips.find((chip) => chip === TxTitle.chipPending) ?? chips.find((chip) => chip !== null) ?? null;
}

/** A group's total in duffs, added exactly; `null` when a member or the total is past what a
 * number holds exactly, so the group shows no total rather than a rounded one. */
export function groupAmount(rows: TxRow[]): number | null {
  if (!rows.every((row) => Number.isSafeInteger(row.amount))) return null;
  const sum = rows.reduce((total, row) => total + BigInt(row.amount), 0n);
  const max = BigInt(Number.MAX_SAFE_INTEGER);
  return sum <= max && sum >= -max ? Number(sum) : null;
}

/** The transaction failed: the chip is in the danger tone, the title secondary, the icon an error. */
export function isProblem(status: TxStatusInfo): boolean {
  return status.kind === "conflicted" || status.kind === "abandoned" || status.kind === "notAccepted";
}

export function trailingStatus(status: TxStatusInfo): string | null {
  return status.kind === "immature" ? TxTitle.locked : null;
}

/** A payment to yourself or an internal move (the icon and chip). */
export function isInternal(type: TxTypeName): boolean {
  return type === "sendToSelf" || type === "platformTransfer";
}

/** The amount a row or detail shows. A self-send shows its fee as money that left; only a pool move
 * between the wallet's own pools is shown unsigned. */
export function shownAmount(amount: number, type: TxTypeName): { duffs: number; signed: boolean } {
  return type === "platformTransfer" ? { duffs: Math.abs(amount), signed: false } : { duffs: amount, signed: true };
}

export function isMixing(type: TxTypeName): boolean {
  return MIXING.includes(type);
}

/** Collapses adjacent CoinJoin rows in a day card; callers keep table/CSV rows unchanged. A
 * conflicted, abandoned or refused row stays on its own so its warning is never folded away. */
export function groupConsecutiveMixing(rows: TxRow[]): (TxRow | MixingGroup)[] {
  const grouped: (TxRow | MixingGroup)[] = [];
  let run: TxRow[] = [];
  const flush = () => {
    if (run.length === 1) grouped.push(run[0]);
    else if (run.length > 1) grouped.push({ kind: "mixingGroup", rows: run });
    run = [];
  };
  for (const row of rows) {
    if (isMixing(row.txType) && !isProblem(row.status)) run.push(row);
    else {
      flush();
      grouped.push(row);
    }
  }
  flush();
  return grouped;
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
  return t("components.date.at", { date, time: timeOfDay(d, locale) });
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
