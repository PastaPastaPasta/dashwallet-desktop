import type { Tree } from "../core";

export const pay = {
  evidenceUnreadable: "Dash Wallet can't read some of this wallet's payment records, so sending is paused. Try again in a moment, or restart Dash Wallet.",
} as const satisfies Tree;
