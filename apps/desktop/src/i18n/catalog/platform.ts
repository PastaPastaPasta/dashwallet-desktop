import type { Tree } from "../core";

/** The Platform addresses' engine refusals that no one screen owns. */
export const platform = {
  /** The engine refused a send or move from the Platform addresses on mainnet (`platform.feature_off`, `platform_address_sends`). */
  mainnetSendOff: "This version sends from Platform addresses on test networks only.",
  /** Platform's structure pre-check refused a move to the Dash wallet, a send to a Platform address or a
   * top-up from Platform before anything was signed: nothing was sent and no balance changed. */
  preflight: {
    /** Only while the network proved a protocol version above every one this build plans at. */
    rulesUnsupported:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them. Nothing was sent, and your balance is unchanged. Update Dash Wallet, then try again.",
    refused: "Dash Platform would refuse this transfer as Dash Wallet built it, so nothing was sent and your balance is unchanged. If this keeps happening, update Dash Wallet.",
    /** The same refusals of the exact retry of a send that still holds: the earlier payment may land. */
    refusedRetry:
      "Dash Platform would refuse this attempt as Dash Wallet built it, so this attempt sent nothing. Your earlier payment may still go through, so check Activity before you pay again. If this keeps happening, update Dash Wallet.",
    rulesUnsupportedRetry:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't send this under them, so this attempt sent nothing. Your earlier payment may still go through. Update Dash Wallet, and check Activity before you pay again.",
    /** The same refusals of the move out of a top-up that still holds: the earlier top-up may land. */
    refusedHeldTopUp:
      "Dash Platform would refuse this move as Dash Wallet built it, so this attempt sent nothing. Your earlier top-up may still go through, so check Activity before you try again. If this keeps happening, update Dash Wallet.",
    rulesUnsupportedHeldTopUp:
      "Dash Platform's rules have changed, and this version of Dash Wallet can't make this move under them, so this attempt sent nothing. Your earlier top-up may still go through. Update Dash Wallet, and check Activity before you try again.",
  },
} as const satisfies Tree;
