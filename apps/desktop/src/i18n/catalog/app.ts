import type { Tree } from "../core";

export const app = {
  /** The splash while the engine opens the network, and the window before the wallets are read. */
  loadingWallet: "Loading wallet…",
  tryAgain: "Try again",
  walletsFailed: "Could not load the wallets",
  /** The splash when the network cannot be opened. `error` is the engine's message. */
  openFailure: {
    tooNew: "This wallet file was written by a newer version of Dash Wallet. Open it with that version; nothing was changed. ({error})",
    failed: "Dash Wallet could not open the wallet: {error}",
  },
  /** Instead of the wallet when the session before (a close, a network switch) still holds its data. */
  storageInUse: {
    title: "Dash Wallet is still closing your wallet data",
    text: "Dash Wallet hasn't finished closing the wallet data it had open before, so it can't open it again yet. Your wallets and funds aren't affected. Try again in a moment, or restart Dash Wallet to finish.",
    /** When trying again can't help until a restart. */
    textRestartOnly: "Dash Wallet hasn't finished closing the wallet data it had open before, so it can't open it again yet. Your wallets and funds aren't affected. Restart Dash Wallet to finish.",
    tryAgain: "Try again",
    restart: "Restart Dash Wallet",
  },
  /** File ▸ Open URI… */
  openUri: {
    title: "Open URI",
    field: "Payment request or link",
    open: "Open",
  },
  about: {
    title: "About Dash Wallet",
    version: "Version",
    engine: "Engine",
    network: "Network",
  },
  /** A lock the engine refused, as a toast. `error` is the engine's message. */
  lockFailed: "Could not lock: {error}",
  /** A command this build of dw-app does not have, where the screen needs it. */
  unavailableInBuild: "This is not available in this build.",
  /** The placeholder text of a route that is not built yet. */
  notYet: {
    explore: "Explore is not available yet.",
  },
  /** Why a link was not opened. */
  link: {
    /** `error` is the engine's message. */
    failed: "Could not open the link: {error}",
    whileWiping: "The link was not opened: close “Delete all wallet data?” first",
    addressRequest: "Dash Wallet does not hand addresses to other apps",
    dashConnect: "DashConnect links are not supported",
    integration: "Exchange links are not supported",
    paymentRequest: "Payment requests (BIP70) are not supported",
    notALink: "This is not a link Dash Wallet can open",
    tooLong: "This link is too long to open",
    notSaved: "This invitation could not be saved. Open the link again once Dash Wallet has started",
    payment: {
      doubleSlash: "This payment link starts with dash:// — Dash links start with dash: alone",
      notDashUri: "This is not a Dash payment link",
      unparsable: "This payment link could not be read",
      bip70Unsupported: "Payment requests (BIP70) are not supported",
      invalidAddress: "The address in this payment link is not valid on this network",
      invalidAmount: "The amount in this payment link is not valid",
    },
  },
  /** The links a window kept for later ("Links you opened"). */
  savedLinks: {
    title: "Links you opened",
    intro: "You opened these links before your wallet was ready. Open one now, or remove it.",
    open: "Open",
    remove: "Remove",
    later: "Later",
    /** The toast for a link kept while there is no wallet yet. */
    saved: "Link saved. It opens once your wallet is set up.",
    /** The toast for an invitation opened during onboarding. */
    invitationSaved: "Invitation saved. It is waiting for you once your wallet is set up.",
    gone: "This link is no longer saved.",
    loadFailed: "Could not read the saved links.",
    /** The stash keeps an invitation an open claim holds. */
    held: "This invitation is being claimed. Finish that claim first.",
    kind: { payment: "Payment request", contact: "Contact", invitation: "DashPay invitation" },
    /** When the link was kept. */
    opened: {
      justNow: "Opened just now",
      minutes: { one: "Opened {count} minute ago", other: "Opened {count} minutes ago" },
      hours: { one: "Opened {count} hour ago", other: "Opened {count} hours ago" },
      yesterday: "Opened yesterday",
      days: { one: "Opened {count} day ago", other: "Opened {count} days ago" },
    },
  },
  /** A file call that failed. */
  file: {
    dialog: "The file dialog could not be shown.",
    tooLarge: "The file is too large.",
    read: "The file could not be read.",
    write: "The file could not be saved.",
  },
  /** A spend refused while the wallet's filter scan is replayed. */
  rescan: {
    running: "The wallet is catching up with the network. You can send once it has finished.",
    interrupted: "The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.",
  },
  /** Home and Pay, while the first start after an update replays this wallet from its birth. */
  upgradeReplay: {
    label: "Checking history after the update",
    text: "Checking this wallet's history after the update. This version rescans it once from its birth to find CoinJoin payments that earlier versions may have missed. You can send when it finishes, usually within a few minutes. A long history takes longer.",
  },
  /** The confirm sheets when the fees are a tenth of the amount or more. */
  feeWarning: {
    most: "Most of this goes to fees.",
    percent: "Fees are {percent}% of this amount.",
  },
  /** Amount entry in fiat. `currency` is a currency code ("EUR"). */
  fiat: {
    toggle: "Enter the amount in {unit}",
    notFresh: "Prices are over 30 minutes old: enter the amount in DASH.",
    unavailable: "Prices unavailable: enter the amount in DASH.",
    backToDash: "Prices are no longer current, so the amount is in DASH again.",
    notKept: "The amount in {currency} could not be converted, so no amount is set. Enter it in DASH.",
    tooLarge: "That amount is too large to convert to DASH.",
    /** `example` is a number as typed: "12.50", or "1250" for a currency with no minor unit. */
    invalid: "Enter an amount in {currency}, for example {example}.",
  },
} as const satisfies Tree;
