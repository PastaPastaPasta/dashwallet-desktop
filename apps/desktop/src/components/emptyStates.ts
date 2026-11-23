// Copy and artwork for screens that have nothing to list yet. Each row is one
// illustration, one sentence, and at most one next step. The screen wires the step.

import { hasCode } from "../api/backend";
import { t } from "../i18n";
import { en } from "../i18n/en";
import type { EmptyArtId } from "./emptyArt";

export interface EmptyAction {
  /** Stable id of the one button. */
  testId: string;
  label: string;
}

export interface EmptyCopy {
  art: EmptyArtId;
  title: string;
  message: string;
  action: EmptyAction | null;
}

type EmptyText = Exclude<keyof typeof en.components.empty, "action">;

interface Step {
  testId: string;
  action: keyof typeof en.components.empty.action;
}

/** A row whose words are read from the catalog when the screen reads them. */
function empty<A extends EmptyArtId>(art: A, text: EmptyText, step: Step): { art: A; title: string; message: string; action: EmptyAction };
function empty<A extends EmptyArtId>(art: A, text: EmptyText, step: null): { art: A; title: string; message: string; action: null };
function empty(art: EmptyArtId, text: EmptyText, step: Step | null): EmptyCopy {
  return {
    art,
    get title() {
      return t(`components.empty.${text}.title`);
    },
    get message() {
      return t(`components.empty.${text}.message`);
    },
    action: step && {
      testId: step.testId,
      get label() {
        return t(`components.empty.action.${step.action}`);
      },
    },
  };
}

export const EMPTY = {
  homeHistory: empty("activity", "noTransactions", { testId: "history-empty-receive", action: "receive" }),
  activity: empty("activity", "noTransactions", { testId: "tx-empty-receive", action: "receive" }),
  historySyncing: empty("activity", "historySyncing", null),
  historyStarting: empty("activity", "historyStarting", null),
  historyStopped: empty("activity", "historyStopped", null),
  historyUnreadable: empty("activity", "historyUnreadable", { testId: "history-recheck", action: "tryAgain" }),
  activityUnreadable: empty("activity", "historyUnreadable", { testId: "tx-recheck", action: "tryAgain" }),
  activityFiltered: empty("search", "activityFiltered", { testId: "tx-empty-clear", action: "clearFilters" }),
  contacts: empty("contacts", "contacts", { testId: "contacts-empty-search", action: "addContact" }),
  notifications: empty("notifications", "notifications", { testId: "notifications-add-contact", action: "addContact" }),
  notificationsNoIdentity: empty("notifications", "notificationsNoIdentity", { testId: "notifications-join", action: "join" }),
  addressSend: empty("address", "addressSend", { testId: "address-book-empty-add", action: "addAddress" }),
  addressReceive: empty("address", "addressReceive", { testId: "address-book-empty-receive", action: "receive" }),
  addressSearch: empty("search", "addressSearch", { testId: "address-book-empty-clear", action: "clearSearch" }),
  coins: empty("coins", "coins", { testId: "coins-empty-receive", action: "receive" }),
  coinsStarting: empty("coins", "coinsStarting", null),
  coinsSyncing: empty("coins", "coinsSyncing", null),
  coinsStopped: empty("coins", "coinsStopped", null),
  coinsUnreadable: empty("coins", "coinsUnreadable", { testId: "coins-recheck", action: "tryAgain" }),
  coinsHidden: empty("coins", "coinsHidden", { testId: "coins-empty-show", action: "showAllCoins" }),
  peers: empty("peers", "peers", null),
  join: empty("username", "join", { testId: "contacts-join-button", action: "join" }),
  wallet: empty("wallet", "wallet", null),
} as const satisfies Record<string, EmptyCopy>;

export type EmptyId = keyof typeof EMPTY;

/** Whether an empty history or coin list is final. Settled only when the engine says caught up.
 * Starting: no reading yet, or Core is not running and has not seen a block. Syncing: Core is
 * running and not caught up. Stopped: Core is not running after it had seen the chain. Unreadable:
 * the sync status was asked for and never arrived. */
export type ListArrival = "starting" | "syncing" | "stopped" | "unreadable" | "settled";

export function listArrival(input: {
  status?: { running: boolean; caughtUp: boolean; tipHeight: number | null } | null;
  failed: boolean;
}): ListArrival {
  const status = input.status ?? null;
  if (!status) return input.failed ? "unreadable" : "starting";
  if (status.caughtUp) return "settled";
  if (status.running) return "syncing";
  return status.tipHeight == null ? "starting" : "stopped";
}

/** The network's raw catch-up, then this wallet's own filter checkpoint.
 * `scanHeight` is `undefined` while that read is still out, `null` when the wallet has no scan state. */
export function catchUpArrival(input: {
  status?: { running: boolean; caughtUp: boolean; tipHeight: number | null } | null;
  failed: boolean;
  scanPending: boolean;
  scanFailed: boolean;
  scanHeight: number | null | undefined;
}): ListArrival {
  const network = listArrival({ status: input.status, failed: input.failed });
  if (network !== "settled") return network;
  if (input.scanPending) return "starting";
  if (input.scanFailed) return "unreadable";
  const height = input.scanHeight ?? null;
  const tip = input.status?.tipHeight ?? null;
  if (height == null) return "starting";
  if (tip != null && height < tip) return "syncing";
  return "settled";
}

/** One source on a list. Absent commands and unsupported features are omitted, not passed as sources. */
export type ListSource = {
  pending: boolean;
  failed: boolean;
  hasSnapshot: boolean;
  rowCount: number;
  /** A read is out for a source that has a snapshot: an event made it stale, or a retry. */
  refreshing?: boolean;
};

export function isUnsupportedOptionalSource(error: unknown): boolean {
  return (
    hasCode(error, "shielded.not_active") ||
    hasCode(error, "platform.feature_off") ||
    hasCode(error, "not_implemented") ||
    hasCode(error, "platform.not_implemented")
  );
}

export function optionalHistorySource(
  q: { isPending: boolean; isError: boolean; isSuccess: boolean; isFetching: boolean; data: unknown; error?: unknown },
  rowCount: number,
): ListSource | null {
  if (q.isSuccess && q.data === null) return null;
  if (q.isError && q.data === undefined && isUnsupportedOptionalSource(q.error)) return null;
  return {
    pending: q.isPending,
    failed: q.isError,
    hasSnapshot: q.data !== undefined,
    rowCount: q.data == null ? 0 : rowCount,
    refreshing: q.isFetching,
  };
}

/** One list, decided once, for the body and every control that depends on it.
 * `incomplete` rows are usable, but the wallet has not caught up, a visible source has not answered
 * yet, or a source's last read failed. */
export type ListDisplay =
  | { kind: "rows"; incomplete: boolean }
  | { kind: "arriving"; why: "pending" | ListArrival }
  | { kind: "failed" }
  | { kind: "empty" };

const sourceWaiting = (s: ListSource) => !s.hasSnapshot && !s.failed;
/** A source whose last read failed and that has nothing to show for it: no snapshot, or an empty
 * one that the failed read might have filled. */
export const sourceGap = (s: ListSource) => s.failed && (!s.hasSnapshot || s.rowCount === 0);

/** Until the primary source first answers, the list is loading. Rows already in hand stay,
 * including after a refetch fails, and a first-read failure stays with them. A confirmed empty
 * result — every visible source answered, and the wallet has caught up — is the only kind that
 * may offer the empty card or a count. An optional source still out is still arriving. A rejected
 * read with no snapshot is a failure, not an empty list, and so is an empty snapshot whose last
 * read failed: the read that failed may be the one with the rows. An empty snapshot being read
 * again (an event said it changed) is still arriving until that read is back. */
export function listDisplay(input: {
  rowCount: number;
  pending: boolean;
  failed: boolean;
  hasSnapshot: boolean;
  refreshing?: boolean;
  arrival: ListArrival;
  /** Optional sources that are actually on this list. Omit a missing command (`null`). */
  extras?: ListSource[];
}): ListDisplay {
  const primary: ListSource = {
    pending: input.pending,
    failed: input.failed,
    hasSnapshot: input.hasSnapshot,
    rowCount: input.hasSnapshot ? input.rowCount : 0,
    refreshing: input.refreshing,
  };
  const sources = [primary, ...(input.extras ?? [])];
  // Optional rows alone, before the first Core answer, would move under the pointer when Core's land.
  if (sourceWaiting(primary)) return { kind: "arriving", why: "pending" };
  const rows = sources.reduce((n, s) => n + (s.hasSnapshot ? s.rowCount : 0), 0);
  if (rows > 0) return { kind: "rows", incomplete: input.arrival !== "settled" || sources.some((s) => !s.hasSnapshot || s.failed) };
  if (sources.some(sourceGap)) return { kind: "failed" };
  if (sources.some(sourceWaiting) || input.arrival !== "settled") {
    const why = sources.some(sourceWaiting) && input.arrival === "settled" ? "syncing" : input.arrival;
    return { kind: "arriving", why: why === "settled" ? "syncing" : why };
  }
  if (sources.some((s) => s.refreshing)) return { kind: "arriving", why: "pending" };
  return { kind: "empty" };
}

/** The zero count and the "nothing to export" line are real only for a complete row list, or for
 * a filter that matched nothing after every source had answered. An unfiltered empty card already
 * says so. A list still catching up, with a source still out, or whose last read failed, is not
 * counted. */
export function listShowsCount(display: ListDisplay, filtered: boolean): boolean {
  return (display.kind === "rows" && !display.incomplete) || (display.kind === "empty" && filtered);
}

/** Contacts. `empty` is the card that carries Add contact: the header button hides only then. */
export type ContactsDisplay = { kind: "rows" | "arriving" | "failed" | "empty" | "search" };

export function contactsDisplay(input: {
  pending: boolean;
  failed: boolean;
  hasSnapshot: boolean;
  rowCount: number;
  updating: boolean;
  searching: boolean;
}): ContactsDisplay {
  if (input.pending) return { kind: "arriving" };
  if (input.failed && !input.hasSnapshot) return { kind: "failed" };
  if (input.rowCount > 0) return { kind: "rows" };
  if (input.updating) return { kind: "arriving" };
  if (input.searching) return { kind: "search" };
  return { kind: "empty" };
}

/** The header's Add contact stays until the empty card that replaces it is what the page renders. */
export function contactsHeaderAdd(display: ContactsDisplay): boolean {
  return display.kind !== "empty";
}

const HISTORY_WAITING = {
  starting: EMPTY.historyStarting,
  syncing: EMPTY.historySyncing,
  stopped: EMPTY.historyStopped,
  unreadable: EMPTY.historyUnreadable,
} as const satisfies Record<Exclude<ListArrival, "settled">, EmptyCopy>;

const COINS_WAITING = {
  starting: EMPTY.coinsStarting,
  syncing: EMPTY.coinsSyncing,
  stopped: EMPTY.coinsStopped,
  unreadable: EMPTY.coinsUnreadable,
} as const satisfies Record<Exclude<ListArrival, "settled">, EmptyCopy>;

/** Home or Activity with nothing in history yet. Only a caught-up wallet offers Receive; one whose
 * catch-up could not be read offers to read it again. */
export function historyEmptyCopy(input: { arrival: ListArrival; home: boolean }): EmptyCopy {
  if (input.arrival === "unreadable" && !input.home) return EMPTY.activityUnreadable;
  if (input.arrival !== "settled") return HISTORY_WAITING[input.arrival];
  return input.home ? EMPTY.homeHistory : EMPTY.activity;
}

/** Coin Selection with no coins in the snapshot. The CoinJoin visibility filter is separate. */
export function coinsEmptyCopy(arrival: ListArrival): EmptyCopy {
  if (arrival !== "settled") return COINS_WAITING[arrival];
  return EMPTY.coins;
}

/** What Activity applied: a category, search, date range or minimum amount. An untouched list is not filtered. */
export function activityFiltersApplied(filter: { categoryId: string; text: string; date: string; minAmount?: number }): boolean {
  return filter.categoryId !== "all" || filter.text !== "" || filter.date !== "all" || filter.minAmount !== undefined;
}

export type AddressBookStep = "add" | "receive" | "clear";

export interface AddressBookEmpty extends EmptyCopy {
  step: AddressBookStep;
}

/** Which address-book empty to show. `null` when the list has rows. The step says which handler to wire. */
export function addressBookEmpty(input: { search: string; purpose: "send" | "receive"; count: number }): AddressBookEmpty | null {
  if (input.count !== 0) return null;
  if (input.search !== "") return { ...EMPTY.addressSearch, step: "clear" };
  return input.purpose === "send" ? { ...EMPTY.addressSend, step: "add" } : { ...EMPTY.addressReceive, step: "receive" };
}

/** How far the wallet list got. Tools offers a new wallet only once that list has loaded and is empty.
 * A failed refetch keeps the list it had: only a list never read is an error. */
export function walletListGate(q: { isPending: boolean; isError: boolean; data?: readonly unknown[] }): "loading" | "error" | "empty" | "ready" {
  if (q.isPending) return "loading";
  if (q.data === undefined) return q.isError ? "error" : "loading";
  return q.data.length === 0 ? "empty" : "ready";
}

/** After a wallet is saved, onboarding may close only once `savedId` is in the list it read.
 * A failed refetch keeps the list it had, so a list that already contains the wallet still counts.
 * The empty list from before the save does not: that read has to be reported and tried again. */
export function walletReadyToOpen(savedId: string | null, list: readonly { walletId: string }[] | undefined): boolean {
  if (!list) return false;
  if (savedId) return list.some((w) => w.walletId === savedId);
  return list.length > 0;
}
