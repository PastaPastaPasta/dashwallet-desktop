import type { Tree } from "../core";

export const activity = {
  /** Transaction details ▸ Details (QT-090, IOS-032). */
  detail: {
    rawTransaction: "Raw transaction",
    /** The copy button's noun: "Copy raw transaction". */
    rawTransactionNoun: "raw transaction",
  },
} as const satisfies Tree;
