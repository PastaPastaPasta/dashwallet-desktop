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
  },
} as const satisfies Tree;
