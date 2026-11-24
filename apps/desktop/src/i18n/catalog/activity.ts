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
} as const satisfies Tree;
