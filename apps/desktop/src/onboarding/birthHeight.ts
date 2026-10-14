// A wallet creation date as a conservative scan start height (QT-105, U-02 D7).
//
// The estimate assumes 200 s per block (the target is 157.5 s), so it lands below the real
// height, and subtracts one more week. Starting early only costs scan time; starting late would
// miss transactions. Devnets and regtest always start at 0.

import type { NetworkInfo } from "../api/generated/NetworkInfo";

const SECONDS_PER_BLOCK = 200;
const SAFETY_MARGIN_SECS = 7 * 24 * 3600;
/** Genesis block times (Dash Core chainparams). */
const GENESIS_SECS: Partial<Record<NetworkInfo["kind"], number>> = { mainnet: 1_390_095_618, testnet: 1_390_666_206 };

export function birthHeightFor(date: Date, network: NetworkInfo["kind"]): number {
  const genesis = GENESIS_SECS[network];
  if (genesis === undefined) return 0;
  const elapsed = date.getTime() / 1000 - genesis - SAFETY_MARGIN_SECS;
  return elapsed > 0 ? Math.min(0xffff_ffff, Math.floor(elapsed / SECONDS_PER_BLOCK)) : 0;
}
