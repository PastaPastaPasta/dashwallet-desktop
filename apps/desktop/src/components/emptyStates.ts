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
  coinsHidden: {
    art: "coins",
    title: "All your coins are mixed",
    message: "Mixed coins are hidden from this list.",
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

/** Home or Activity with nothing in history yet. Syncing has no button; a caught-up wallet offers Receive. */
export function historyEmptyCopy(input: { syncing: boolean; home: boolean }): EmptyCopy {
  if (input.syncing) return EMPTY.historySyncing;
  return input.home ? EMPTY.homeHistory : EMPTY.activity;
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

/** How far the wallet list got. Tools offers a new wallet only once that list has loaded and is empty. */
export function walletListGate(state: "pending" | "error" | "success", count: number): "loading" | "error" | "empty" | "ready" {
  if (state === "pending") return "loading";
  if (state === "error") return "error";
  return count === 0 ? "empty" : "ready";
}
