import type { Tree } from "../core";

export const masternodes = {
  withdraw: {
    /** An evonode credit withdrawal whose identity data the wallet could not verify (H-09). */
    unverified: "Dash Wallet can't check this masternode's details yet. Try again in a few minutes.",
    /** The destination row while the engine still checks where the withdrawal pays. */
    checking: "Checking…",
  },
} as const satisfies Tree;
