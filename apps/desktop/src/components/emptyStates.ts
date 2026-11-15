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
  contactRequests: {
    art: "requests",
    title: "No contact requests",
    message: "When someone asks to connect, the request shows here.",
    action: { testId: "contact-requests-add", label: "Add a contact" },
  },
  notifications: {
    art: "notifications",
    title: "No notifications yet",
    message: "Payments and updates from your contacts show here.",
    action: null,
  },
  notificationsNoIdentity: {
    art: "notifications",
    title: "No notifications",
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
    action: { testId: "address-book-empty-receive", label: "Receive" },
  },
  addressSearch: {
    art: "search",
    title: "No entry matches the search",
    message: "Try a different address or label.",
    action: { testId: "address-book-empty-clear", label: "Clear search" },
  },
  receiveRequests: {
    art: "payment",
    title: "No payment requests yet",
    message: "Ask for a specific amount, label or message.",
    action: { testId: "requests-empty-create", label: "Create a request" },
  },
  coinjoin: {
    art: "coinjoin",
    title: "Not Started",
    message: "Mix coins so payments are harder to trace.",
    action: { testId: "coinjoin-action", label: "Start Mixing" },
  },
  coins: {
    art: "coins",
    title: "This wallet has no coins yet",
    message: "Receive Dash and the coins show here.",
    action: { testId: "coins-empty-receive", label: "Receive Dash" },
  },
  coinsHidden: {
    art: "coins",
    title: "Every coin of this wallet is a CoinJoin coin",
    message: "Show all coins lists them.",
    action: { testId: "coins-empty-show", label: "Show all coins" },
  },
  peers: {
    art: "peers",
    title: "No peers are connected",
    message: "Look for other peers on the network.",
    action: { testId: "peers-empty-rotate", label: "Change peers" },
  },
  payContacts: {
    art: "contacts",
    title: "No contacts yet",
    message: "Add a contact to pay them by name.",
    action: { testId: "pay-picker-add", label: "Add a contact" },
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
    message: "Create or restore a wallet to use this.",
    action: { testId: "no-wallet-create", label: "Create a wallet" },
  },
} as const satisfies Record<string, EmptyCopy>;

export type EmptyId = keyof typeof EMPTY;

/** What Activity applied: a category, search, date range or minimum amount. An untouched list is not filtered. */
export function activityFiltersApplied(filter: { categoryId: string; text: string; date: string; minAmount?: number }): boolean {
  return filter.categoryId !== "all" || filter.text !== "" || filter.date !== "all" || filter.minAmount !== undefined;
}

/** Which address-book empty to show. `null` when the list has rows. */
export function addressBookEmpty(input: { search: string; purpose: "send" | "receive"; count: number }): EmptyCopy | null {
  if (input.count !== 0) return null;
  if (input.search !== "") return EMPTY.addressSearch;
  return input.purpose === "send" ? EMPTY.addressSend : EMPTY.addressReceive;
}
