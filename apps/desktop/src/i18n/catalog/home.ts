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
  /** What to do about the coins a dropped payment holds. A wallet a recovery phrase restores is
   * removed and added again, scanning the whole chain (the …Single texts when it is the only open
   * wallet); one added from a seed without a phrase, or from a wallet.dat with legacy address
   * ranges, is kept (`keep…`). While a transaction that uses a held coin isn't final, any other
   * waits (`waiting`). The labels are the app's own. */
  heldUnfinished: {
    waiting:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. A transaction that uses some of these coins isn't final on the network yet. When it is, Dash Wallet shows here how to get back any coins the payment didn't spend. Until then, don't remove or restore {wallet}.",
    wayOut:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase, and its BIP39 passphrase if the wallet was created with one. That isn't the passphrase you unlock Dash Wallet with: the same words without it restore a different wallet. Check too whether you added {wallet} as {coreCompat}, because you need to choose the same again. Keep your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Removing {wallet} also unlocks any coins you locked with {lockUnspent} in {tools} ▸ {coinSelection}, so note which they are and lock them again before you send or mix. Then, in {settings} ▸ {walletTab}, remove {wallet} (if it asks you to open another wallet first, do that), and restore it with {addWallet} and its phrase. On {optionsTitle}, enter the BIP39 passphrase if it has one, set {coreCompat} the same way as before, and under {scanFrom}, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins {wallet} has already spent can show as unspent, so don't send anything from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    wayOutSingle:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase, and its BIP39 passphrase if the wallet was created with one. That isn't the passphrase you unlock Dash Wallet with: the same words without it restore a different wallet. Check too whether you added {wallet} as {coreCompat}, because you need to choose the same again. Keep your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Removing {wallet} also unlocks any coins you locked with {lockUnspent} in {tools} ▸ {coinSelection}, so note which they are and lock them again before you send or mix. Then, in {settings} ▸ {walletTab}, remove {wallet}. If it asks you to open another wallet first, open it, remove {wallet}, and restore it with {addWallet} and its phrase. Otherwise Dash Wallet shows {welcomeTitle} once {wallet} is removed: choose {welcomeRestore} and enter its phrase. On {optionsTitle}, enter the BIP39 passphrase if it has one, set {coreCompat} the same way as before, and under {scanFrom}, choose The beginning. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins {wallet} has already spent can show as unspent, so don't send anything from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    wayOutWatchOnly:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Removing {wallet} also unlocks any coins you locked with {lockUnspent} in {tools} ▸ {coinSelection}, so note which they are and lock them again before you send or mix. Then, in {settings} ▸ {walletTab}, remove {wallet} and add it again with {addWatchOnly} and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins {wallet} has already spent can show as unspent, so don't create a payment from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through.",
    wayOutWatchOnlySingle:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. To get back any coins the payment didn't spend, you'll remove {wallet} and add it again as a watch-only wallet. First make sure you have its extended public key (the wallet that signs for it can show it) and your own copies of any masternode keys or shared-masternode standbys you added, because the key doesn't restore them. Removing {wallet} also unlocks any coins you locked with {lockUnspent} in {tools} ▸ {coinSelection}, so note which they are and lock them again before you send or mix. Then, in {settings} ▸ {walletTab}: add another wallet (a new one is fine), remove {wallet}, and add it again with {addWatchOnly} and that key. It scans the whole chain again, which can take a long time. Coins the payment didn't spend come back once it has finished syncing. Until then, coins {wallet} has already spent can show as unspent, so don't create a payment from it until syncing finishes: the network won't accept a payment that uses one of them. If the unfinished payment was replaced by a newer version that hasn't confirmed yet, a coin only that newer version uses can also come back as unspent. Spending it conflicts with that payment, and only one of the two goes through. You can remove the extra wallet afterwards.",
    keepRaisedLookahead:
      "Keep {wallet}. A recovery phrase alone may leave out addresses this wallet scanned with a larger lookahead. Dash Wallet still holds the coins this payment was using. The rest of the wallet works as usual.",
    keepNoPhrase:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. Keep {wallet}: don't remove it to get them back. It was added from a seed without a recovery phrase, so it can't be restored from a phrase, and removing it deletes its keys from this computer. Coins the payment didn't spend stay held in {wallet}: they're still yours, but this version won't spend them. The rest of {wallet} works as usual.",
    keepLegacy:
      "Dash Wallet stopped this payment but still holds the coins it was using, and this version can't release them on its own. Keep {wallet}: don't remove it to get them back. It was added from a Dash Core wallet.dat that uses older addresses a recovery phrase doesn't restore, so removing and restoring it would leave some of its coins out. Coins the payment didn't spend stay held in {wallet}: they're still yours, but this version won't spend them. The rest of {wallet} works as usual.",
  },
  /** The Shielded row's subtitle while a payment that may still go through holds part of it.
   * `amount` is the held total with its unit; `count` the payments holding it. */
  shielded: {
    held: {
      one: "{amount} held for a payment",
      other: "{amount} held for {count} payments",
    },
  },
  shortcuts: {
    label: "Shortcuts",
    receive: "Receive",
    send: "Send",
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
