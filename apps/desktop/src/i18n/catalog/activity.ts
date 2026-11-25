import type { Tree } from "../core";

export const activity = {
  abandon: {
    spending: "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.",
    notSpending: "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.",
    unknownInputs:
      "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again. If this is a payment you sent, sending it again could pay twice. If Dash Wallet sent it since it last started, it may keep re-sending it for up to a day. To stop that, restart Dash Wallet.",
    rescanFirst: "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.",
    settledChild:
      "This transaction can't be abandoned because a later transaction that depends on it is already locked or confirmed.",
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
  },
} as const satisfies Tree;
