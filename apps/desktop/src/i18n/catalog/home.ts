import type { Tree } from "../core";

export const home = {
  title: "Home",
  hero: {
    label: "Balance",
    show: "Click to show balance",
    hide: "Click to hide balance",
    unavailableTitle: "Not available until sync completes",
    unavailable: "Balance unavailable",
    /** A breakdown amount that could not be read; shown as "—", said to screen readers. */
    notAvailable: "not available",
    /** A breakdown amount still being read; shown as "…", said to screen readers. */
    loading: "loading",
  },
  /** The hero's line above the balance. */
  sync: {
    syncing: "Still syncing, balance may change",
    stopped: "Syncing has stopped, balance may be out of date",
    notStarted: "Not synced yet, balance may change",
    unreadable: "Could not check syncing, balance may change",
    unknownSyncing: "Syncing Balance",
    unknownNotSynced: "Not synced yet",
  },
  /** The breakdown under the hero: each pool's name and what it is. */
  pools: {
    core: { title: "Core", subtitle: "Dash wallet" },
    shielded: { title: "Shielded", subtitle: "Private balance" },
    platform: { title: "Platform", subtitle: "Platform addresses" },
  },
  /** The Core row's subtitle: what is still arriving, and what is held. Parts joined by `separator`. */
  core: {
    pending: "{amount} pending",
    immature: "{amount} immature",
    locked: "{amount} locked",
    separator: " · ",
  },
  shortcuts: {
    label: "Shortcuts",
    receive: "Receive",
    send: "Send",
    scan: "Scan QR",
    backup: "Back up",
    backupWhy: "Backing up the recovery phrase is not available yet",
  },
  history: {
    title: "History",
    /** `percent` has one decimal. */
    syncing: "Syncing {percent}%",
    filter: "Filter",
    hidden: "Recent transactions are hidden in discreet mode",
    failed: "Could not load transactions",
    tryAgain: "Try again",
    loading: "Loading transactions",
    seeAll: "See all transactions",
  },
  /** The Unfinished transfers sheet: a lock's transaction ID, copied. */
  unfinished: { copyTxid: "Copy transaction ID" },
  /** The banner when the network has upgraded past this version (a security notice). */
  advisory: {
    label: "App update available",
    text: "The network has upgraded. Check for an app update.",
    link: "GitHub releases",
    dismiss: "Dismiss",
    openFailed: "Could not open the releases page.",
  },
  /** An unfinished Dash Core export's unencrypted temporary file, found at the start. */
  exportStaging: {
    /** Toast: the start deleted it. */
    deleted: "Dash Wallet deleted an unfinished Dash Core export left from the last time it ran. That file wasn't encrypted. If you still need the export, export again.",
    /** Banner, until dismissed: the start could not delete it; `{path}` is its full path. */
    notDeleted: "Dash Wallet couldn't delete an unfinished Dash Core export left from the last time it ran. That file isn't encrypted and can hold your wallet's private keys. Delete it yourself: {path}",
    label: "Unfinished export",
    dismiss: "Dismiss",
  },
} as const satisfies Tree;
