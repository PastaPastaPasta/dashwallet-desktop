// Home's balance (0.1 release plan, the iOS model): the hero is everything the wallet can spend,
// the Core balance (mixed coins included) plus the shielded one plus the Platform addresses'; the
// breakdown names the three. A pool whose balance is not known (dw-app has no command for it yet,
// or the read failed) is left out of the hero and shows "—" in its row.

import type { Balances } from "../api/generated/Balances";

export type Pool = "core" | "shielded" | "platform";

export interface HomeBalance {
  /** The hero: the pools that are known, in duffs. */
  total: number;
  /** Each pool's balance in duffs; `null`: not known. */
  pools: Record<Pool, number | null>;
}

export function homeBalance(core: Balances, shielded: number | null | undefined, platform: number | null | undefined): HomeBalance {
  const pools = { core: core.total, shielded: shielded ?? null, platform: platform ?? null };
  return { total: pools.core + (pools.shielded ?? 0) + (pools.platform ?? 0), pools };
}

export const POOL_ROWS: { pool: Pool; title: string; subtitle: string }[] = [
  { pool: "core", title: "Core", subtitle: "Dash wallet" },
  { pool: "shielded", title: "Shielded", subtitle: "Private balance" },
  { pool: "platform", title: "Platform", subtitle: "Platform addresses" },
];
