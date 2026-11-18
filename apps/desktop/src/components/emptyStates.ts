// Copy and artwork for screens that have nothing to list yet. Each row is one
// illustration, one sentence, and at most one next step. The screen wires the step.

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

export const EMPTY = {
  homeHistory: {
    art: "activity",
    title: "There are no transactions to display",
    message: "Payments you receive and send show up here.",
    action: { testId: "history-empty-receive", label: "Receive Dash" },
  },
  activity: {
    art: "activity",
    title: "There are no transactions to display",
    message: "Payments you receive and send show up here.",
    action: { testId: "tx-empty-receive", label: "Receive Dash" },
  },
  historySyncing: {
    art: "activity",
    title: "Your history is still syncing",
    message: "Payments show up here as the wallet catches up.",
    action: null,
  },
  historyStarting: {
    art: "activity",
    title: "The wallet is still starting",
    message: "Payments show up here once it has caught up.",
    action: null,
  },
  historyStopped: {
    art: "activity",
    title: "Syncing has stopped",
    message: "Payments show up here once the wallet is syncing again.",
    action: null,
  },
  historyUnreadable: {
    art: "activity",
    title: "Could not tell whether the wallet has caught up",
    message: "Payments show up here once that can be checked.",
    action: null,
  },
  activityFiltered: {
    art: "search",
    title: "Nothing matches these filters",
    message: "Try a different search, or show every transaction.",
    action: { testId: "tx-empty-clear", label: "Clear filters" },
  },
  contacts: {
    art: "contacts",
    title: "No contacts yet",
    message: "Search for a user by their username to send them a contact request.",
    action: { testId: "contacts-empty-search", label: "Add contact" },
  },
  notifications: {
    art: "notifications",
    title: "No notifications yet",
    message: "Contact requests and payments from your contacts show up here.",
    action: { testId: "notifications-add-contact", label: "Add contact" },
  },
  notificationsNoIdentity: {
    art: "notifications",
    title: "No notifications yet",
    message: "Create a username to get contact requests and payments from contacts.",
    action: { testId: "notifications-join", label: "Join DashPay" },
  },
  addressSend: {
    art: "address",
    title: "No sending addresses yet",
    message: "Save an address you pay often.",
    action: { testId: "address-book-empty-add", label: "Add address" },
  },
  addressReceive: {
    art: "address",
    title: "No receiving addresses yet",
    message: "New addresses are created on Receive.",
    action: { testId: "address-book-empty-receive", label: "Receive Dash" },
  },
  addressSearch: {
    art: "search",
    title: "No entry matches the search",
    message: "Try a different address or label.",
    action: { testId: "address-book-empty-clear", label: "Clear search" },
  },
  coins: {
    art: "coins",
    title: "No coins yet",
    message: "Coins you receive show up here.",
    action: { testId: "coins-empty-receive", label: "Receive Dash" },
  },
  coinsStarting: {
    art: "coins",
    title: "The wallet is still starting",
    message: "Coins show up here once it has caught up.",
    action: null,
  },
  coinsSyncing: {
    art: "coins",
    title: "Coins are still syncing",
    message: "Coins you already have show up here as the wallet catches up.",
    action: null,
  },
  coinsStopped: {
    art: "coins",
    title: "Syncing has stopped",
    message: "Coins show up here once the wallet is syncing again.",
    action: null,
  },
  coinsUnreadable: {
    art: "coins",
    title: "Could not tell whether the wallet has caught up",
    message: "Coins show up here once that can be checked.",
    action: null,
  },
  coinsHidden: {
    art: "coins",
    title: "Every coin here is a CoinJoin coin",
    message: "CoinJoin coins are hidden from this list.",
    action: { testId: "coins-empty-show", label: "Show all coins" },
  },
  peers: {
    art: "peers",
    title: "No peers are connected",
    message: "Check your internet connection. The wallet keeps looking for peers.",
    action: null,
  },
  join: {
    art: "username",
    title: "Join DashPay",
    message: "Create a username to add contacts and pay them by name.",
    action: { testId: "contacts-join-button", label: "Join DashPay" },
  },
  wallet: {
    art: "wallet",
    title: "No wallet on this network yet",
    message: "Create or restore a wallet to get started.",
    action: null,
  },
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

/** Home or Activity with nothing in history yet. Only a caught-up wallet offers Receive. */
export function historyEmptyCopy(input: { arrival: ListArrival; home: boolean }): EmptyCopy {
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
