import type { Tree } from "../core";

export const activity = {
  abandon: {
    spending: "Abandon this transaction? Dash Wallet stops counting it, and the coins it spent come back to your balance unless something else has already spent them. If it still goes through, it counts again, so sending the same payment again could pay twice.",
    notSpending: "Abandon this transaction? Dash Wallet stops counting it. If it's locked or confirmed anyway, it counts again.",
    rescanFirst: "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again.",
  },
} as const satisfies Tree;
