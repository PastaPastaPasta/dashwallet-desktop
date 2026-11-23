import type { Tree } from "../core";

export const activity = {
  /** Transaction details ▸ Details (QT-090, IOS-032). */
  detail: {
    rawTransaction: "Raw transaction",
    /** The row's copy button. */
    copyRawTransaction: "Copy raw transaction",
  },
} as const satisfies Tree;
