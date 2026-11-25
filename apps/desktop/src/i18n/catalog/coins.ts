import type { Tree } from "../core";

/** A coin's own state, wherever the coin is listed. */
export const coins = {
  /** A coin kept out of the spendable balance and of spending while the payment that sent it, or
   * one it was built on, may never confirm. Shown on the coin's row and in that payment's details. */
  pendingUnproven:
    "Dash Wallet can't yet tell whether the payment that sent this coin will confirm, so the coin isn't counted in your spendable balance or offered for spending until it does.",
} as const satisfies Tree;
