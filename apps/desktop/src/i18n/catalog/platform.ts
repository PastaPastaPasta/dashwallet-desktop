import type { Tree } from "../core";

/** The Platform addresses' engine refusals that no one screen owns. */
export const platform = {
  /** The engine refused a send or move from the Platform addresses on mainnet (`platform.feature_off`, `platform_address_sends`). */
  mainnetSendOff: "This version sends from Platform addresses on test networks only.",
} as const satisfies Tree;
