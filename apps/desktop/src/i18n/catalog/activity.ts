import type { Tree } from "../core";

export const activity = {
  abandon: {
    spending: "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice.",
    notSpending: "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.",
    unknownInputs:
      "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again. If this is a payment you sent, sending it again could pay twice.",
    rescanFirst: "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.",
    settledChild:
      "This transaction can't be abandoned because a later transaction that depends on it is already locked or confirmed.",
    /** A saved transaction of {wallet} doesn't read; more than one wallet is open. */
    recordUnreadable:
      "Dash Wallet can't read one of the transaction records in {wallet}. While {wallet} is open, Abandon and Remove Unconfirmed are unavailable in every wallet. Closing it in {settings} ▸ {walletTab} makes them available in the others. To fix it, remove {wallet} there and restore it from its recovery phrase. Its coins come back once the restored wallet syncs. Before you remove it, make sure you have its recovery phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them.",
    /** As `recordUnreadable`, with only {wallet} open. */
    recordUnreadableOnly:
      "Dash Wallet can't read one of the transaction records in {wallet}, so Abandon and Remove Unconfirmed are unavailable until it's fixed. To fix it, you'll remove {wallet} and restore it from its recovery phrase. First make sure you have that phrase and your own copies of any masternode keys or shared-masternode standbys you added, because the phrase doesn't restore them. Then, in {settings} ▸ {walletTab}: add another wallet (a new one is fine), remove {wallet}, and restore it with {addWallet} and its phrase. Its coins come back once it syncs. You can remove the extra wallet afterwards.",
  },
  /** Transaction details ▸ Details (QT-090, IOS-032). */
  detail: {
    rawTransaction: "Raw transaction",
    /** The row's copy button. */
    copyRawTransaction: "Copy raw transaction",
  },
} as const satisfies Tree;
