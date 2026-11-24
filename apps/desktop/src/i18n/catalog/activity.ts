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
