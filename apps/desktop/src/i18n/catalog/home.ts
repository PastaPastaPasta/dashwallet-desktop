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
    /** Coins a dropped payment holds back, every one of known value. */
    heldUnfinished: "{amount} held by an unfinished payment",
    /** As `heldUnfinished`, when some held coins' value is unknown: {amount} sums the known ones. */
    heldUnfinishedAtLeast: "At least {amount} held by an unfinished payment",
    /** No held coin's value is known; {count} is how many are held. */
    heldUnfinishedUnvalued: {
      one: "A coin held by an unfinished payment",
      other: "{count} coins held by an unfinished payment",
    },
    /** The link beside the held line that opens `heldUnfinished.wayOut…`. */
    heldWhatCanIDo: "What can I do?",
  },
  /** How to get back the coins a dropped payment holds: remove {wallet} and add it again, scanning
   * the whole chain. The labels are the app's own: Settings ▸ Wallet, its add buttons, and the
   * restore options' scan choice. The …Single texts are for a lone open wallet, which can't be
   * removed until another is added. */
  heldUnfinished: {
    wayOut:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in {settings} ▸ {walletTab}, remove {wallet} and restore it with {addWallet} and its phrase. On {optionsTitle}, under {scanFrom}, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing.",
    wayOutSingle:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in {settings} ▸ {walletTab}: add another wallet (a new one is fine), remove {wallet}, and restore it with {addWallet} and its phrase. On {optionsTitle}, under {scanFrom}, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. You can remove the extra wallet afterwards.",
    wayOutWatchOnly:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Then, in {settings} ▸ {walletTab}, remove {wallet} and add it again with {addWatchOnly} and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing.",
    wayOutWatchOnlySingle:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Then, in {settings} ▸ {walletTab}: add another wallet (a new one is fine), remove {wallet}, and add it again with {addWatchOnly} and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. You can remove the extra wallet afterwards.",
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
  /** Open wallets whose history starts before the block data this install keeps: their earlier
   * payments are missing until chain data is reset. `wallet` is a wallet's name. */
  historyGap: {
    title: "Older payments aren't shown yet",
    oneWallet: "The history of “{wallet}” starts before block {height}, where this app's block data begins. Its payments before that block aren't shown yet, so its balance may be missing coins.",
    wallets: {
      other: "The history of {count} wallets starts before block {height}, where this app's block data begins. Their payments before that block aren't shown yet, so their balances may be missing coins.",
    },
    action: "Reset chain data and resync downloads it again from the earliest wallet's birth. Your wallets, keys and settings are kept. It takes several minutes, and you can't send until it finishes.",
    notNow: "Not now",
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
