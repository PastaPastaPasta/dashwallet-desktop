import type { Tree } from "../core";

export const activity = {
  abandon: {
    spending: "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.",
    notSpending: "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.",
    unknownInputs:
      "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again. If this is a payment you sent, sending it again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.",
    rescanFirst: "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.",
    /** Beside Abandon on a payment whose coin is pending because it may never confirm
     * (`coins.pendingUnproven`). */
    unprovenParentHint:
      "This payment may never confirm, for example if the coins it spends were already spent elsewhere. If it stays unconfirmed, you can abandon it so Dash Wallet stops showing its pending coin. Coins the payment spent come back once Dash Wallet has checked that they haven't been spent elsewhere. If it confirms anyway, the coin is counted again.",
    /** In an abandoned payment's details while a rescan checks a coin it spent before giving it back. */
    checkingSpentCoins:
      "Dash Wallet is checking the coins this payment spent. Any that haven't been spent elsewhere come back to your balance when the check finishes.",
    /** The engine refused with `rescan_in_progress`: after a chain fork it catches up for about a block. */
    catchingUp: "The wallet is catching up with the network. You can abandon this payment once it has finished.",
    settledChild:
      "This transaction can't be abandoned because a later transaction that depends on it is already locked or confirmed.",
    platformFunding:
      "This payment can't be abandoned, because it pays for a Platform transfer, credit top-up or username registration that hasn't finished. Dash Wallet sends it again until that finishes.",
    /** A saved transaction of {wallet} doesn't read; more than one wallet is open. */
    recordUnreadable:
      "Dash Wallet can't read one of the transaction records in {wallet}, so Abandon and Remove Unconfirmed are unavailable in every wallet until it's fixed. To fix it, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in {settings} ▸ {walletTab}, remove {wallet} and restore it with {addWallet} and its phrase. Its coins come back once it syncs. Wait until it has finished syncing before you use Abandon or Remove Unconfirmed in any wallet.",
    /** As `recordUnreadable`, with only {wallet} open. */
    recordUnreadableOnly:
      "Dash Wallet can't read one of the transaction records in {wallet}, so Abandon and Remove Unconfirmed are unavailable until it's fixed. To fix it, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in {settings} ▸ {walletTab}: add another wallet (a new one is fine), remove {wallet}, and restore it with {addWallet} and its phrase. Its coins come back once it syncs. Wait until it has finished syncing before you use Abandon or Remove Unconfirmed. You can remove the extra wallet afterwards.",
    /** Above the confirm, while the payment holds its hold from before a restore (Q92 H2). */
    restoredHint:
      "This payment was still pending in your wallet data from before the upgrade, and it hasn't confirmed since. Anything you did with it after the upgrade, such as abandoning it, is no longer recorded here.",
    /** Instead of Abandon, while the restored wallet data is still being checked. */
    restoredGate: "Dash Wallet is checking the wallet data it restored from before the upgrade. You can abandon this payment once the wallet has finished syncing.",
  },
  /** A transaction Dash Wallet dropped itself because it can never confirm: its chip and status
   * word, and the line its details show for where the engine says its coins are. */
  status: {
    droppedLabel: "Dropped",
    /** Every coin of the wallet's own it used is back in the balance. */
    dropped:
      "Dash Wallet dropped this payment because it can never confirm: it depends on a coin or payment that was spent or replaced elsewhere. Any of your own coins it used are back in your balance.",
    /** The drop holds its coins; {whatCanIDo} is the link label under Home's held amount. */
    droppedHeld:
      "Dash Wallet dropped this payment because it can never confirm. It can't tell yet whether the coins it used were spent elsewhere, so it holds them back, and they're counted in the held amount on Home. To get back any that weren't spent, open {whatCanIDo} under that amount on Home.",
    /** A running rescan brings its coins back. */
    droppedReleasing:
      "Dash Wallet dropped this payment because it can never confirm. Any of your own coins it used will be back in your balance when the current rescan finishes.",
    /** No scan running brings a coin back; {tools} is the Tools screen, {rescan} its Rescan button. */
    droppedRescan:
      "Dash Wallet dropped this payment because it can never confirm. Some of your own coins it used aren't back in your balance yet. To get them back, open {tools} and run {rescan} from the wallet's birth.",
    /** A coin's funding block is below the wallet's birth; {birthTitle} is Repair's birth field. */
    droppedRescanFrom:
      "Dash Wallet dropped this payment because it can never confirm. Some of your own coins it used aren't back in your balance yet, because they're older than this wallet's birth height. To get them back, open {tools} and set {birthTitle} to {height} or lower.",
    /** A coin came from a transaction that isn't in a block yet. */
    droppedAwaitingFunding:
      "Dash Wallet dropped this payment because it can never confirm. Some of your own coins it used came from a transaction that hasn't confirmed yet. They'll be back in your balance if it confirms.",
    /** The never-valid incoming transaction itself. */
    droppedClaim: "Dash Wallet dropped this transaction because it was never valid, so it can never confirm. It no longer counts toward your balance.",
    /** Home's link under the held amount, until Home names it itself. */
    whatCanIDo: "What can I do?",
  },
  /** The notice of payments dropped because what they spent came from an incoming transaction that
   * was never valid, shown once their coins are back, until dismissed. */
  invalidSource: {
    notice:
      "A payment of {amount} to {recipient} didn't go through. The Dash it used came from an incoming transaction that was never valid, so Dash Wallet dropped the payment.",
    /** Two or more. */
    noticeSeveral: {
      other: "{count} payments didn't go through. The Dash they used came from an incoming transaction that was never valid, so Dash Wallet dropped them.",
    },
    details: "Details",
    dismiss: "Dismiss",
    /** The payment's details, under its status. */
    detail:
      "This payment to {recipient} used Dash that an incoming transaction said was yours. That transaction was never valid, so the payment could never confirm. Nothing left your wallet: any of your own coins the payment used are back in your balance. If {recipient} still needs to be paid, send the payment again.",
  },
  /** Transaction details ▸ Details (QT-090, IOS-032). */
  detail: {
    rawTransaction: "Raw transaction",
    /** The row's copy button. */
    copyRawTransaction: "Copy raw transaction",
  },
  /** The details of a Shielded or Platform-address row, from its own activity record. */
  sourceDetail: {
    title: "Transaction details",
    loading: "Loading details…",
    /** The record left the list while its details were open. */
    gone: "This activity is no longer listed.",
    status: {
      confirmed: "Confirmed",
      pending: "Pending",
      notAccepted: "Not accepted",
    },
    /** The text the sender attached to a Shielded payment. */
    memo: "Memo",
    from: "From",
    to: "To",
    /** The wallet's own Platform address that a Platform row changed. */
    platformAddress: "Platform address",
    fee: "Fee",
    /** A Platform send whose fee is the estimate it was charged. */
    feeEstimated: "Fee (estimated)",
    platformBlock: "Platform block",
    copyAddress: "Copy address",
    /** A Platform payment that may still go through: its way out moves the same funds to the Dash wallet. */
    moveToCore: "Move to Dash wallet",
    /** A Shielded payment Platform refused when it was sent. */
    refusal: {
      /** Its funds ({amount}) stay held until Dash Wallet proves it can no longer go through. */
      held: "Dash Platform didn't accept this payment when it was sent, but it may still accept it later. Until Dash Wallet can prove it no longer can, the Shielded funds it spends ({amount}) stay held. Don't send this payment again: both could go through. If it goes through, Activity shows it as sent. If it can no longer go through, the funds become spendable again. This can take hours, or days when Shielded is quiet.",
      /** As `held`, with no amount known. */
      heldNoAmount:
        "Dash Platform didn't accept this payment when it was sent, but it may still accept it later. Until Dash Wallet can prove it no longer can, the Shielded funds it spends stay held. Don't send this payment again: both could go through. If it goes through, Activity shows it as sent. If it can no longer go through, the funds become spendable again. This can take hours, or days when Shielded is quiet.",
      /** After Dash Wallet proved it can no longer go through. */
      released: "Dash Platform didn't accept this payment, and it can no longer go through, so Dash Wallet has stopped holding the Shielded funds it used. Any of those funds that another payment hasn't spent are spendable again.",
    },
  },
} as const satisfies Tree;
