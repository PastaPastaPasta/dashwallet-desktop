// Home's balance (0.1 release plan, the iOS model): the hero is everything the wallet can spend,
// the Core balance that can be spent (confirmed + unconfirmed, mixed coins included; immature and
// locked funds are not) plus the shielded one plus the Platform addresses'; the
// breakdown names those that hold something. A pool whose balance is not known (dw-app has no
// command for it yet, or the read failed) is left out of the hero and of the breakdown.

import type { Balances } from "../api/generated/Balances";

export type Pool = "core" | "shielded" | "platform";

export interface HomeBalance {
  /** The hero: the pools that are known, in duffs. */
  total: number;
  /** Each pool's balance in duffs; `null`: not known. */
  pools: Record<Pool, number | null>;
}

export function homeBalance(core: Balances, shielded: number | null | undefined, platform: number | null | undefined): HomeBalance {
  const pools = { core: core.confirmed + core.unconfirmed, shielded: shielded ?? null, platform: platform ?? null };
  return { total: pools.core + (pools.shielded ?? 0) + (pools.platform ?? 0), pools };
}

/**
 * The Core row's subtitle: what is still arriving (already in the row's number), then what is in
 * the wallet but not spendable (not in it): immature rewards and locked coins.
 */
export function coreSubtitle(core: Balances, fmt: (duffs: number) => string, fallback: string): string {
  const parts: string[] = [];
  if (core.unconfirmed > 0) parts.push(`${fmt(core.unconfirmed)} pending`);
  if (core.immature > 0) parts.push(`${fmt(core.immature)} immature`);
  if (core.locked > 0) parts.push(`${fmt(core.locked)} locked`);
  return parts.length > 0 ? parts.join(" · ") : fallback;
}

export const POOL_ROWS: { pool: Pool; title: string; subtitle: string }[] = [
  { pool: "core", title: "Core", subtitle: "Dash wallet" },
  { pool: "shielded", title: "Shielded", subtitle: "Private balance" },
  { pool: "platform", title: "Platform", subtitle: "Platform addresses" },
];

/**
 * The breakdown's rows under the hero: Shielded and Platform only while they hold something (an
 * empty pool, or one not known, says nothing a user needs). With neither, Core shows only to say
 * what is pending, immature or locked; otherwise the hero says it all and there is no breakdown.
 */
export function breakdownRows(core: Balances, pools: HomeBalance["pools"]): Pool[] {
  const others = (["shielded", "platform"] as const).filter((p) => (pools[p] ?? 0) > 0);
  const coreNews = core.unconfirmed > 0 || core.immature > 0 || core.locked > 0;
  return others.length > 0 || coreNews ? ["core", ...others] : [];
}

/**
 * The hero amount as the mobile wallets draw it: the whole part and the first two decimals at full
 * size, the remaining decimals smaller. Nothing is rounded; the text is split, not changed.
 */
export function heroDigits(text: string, major = 2): { major: string; minor: string } {
  const dot = text.indexOf(".");
  if (dot < 0) return { major: text, minor: "" };
  return { major: text.slice(0, dot + 1 + major), minor: text.slice(dot + 1 + major) };
}
