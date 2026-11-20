// Home's balance (0.1 release plan, the iOS model): the hero is everything the wallet can spend,
// the Core balance that can be spent (confirmed + unconfirmed, mixed coins included; immature and
// locked funds are not) plus the shielded one plus the Platform addresses'; the breakdown names the
// pools. Only a pool's known balance counts in the hero: one still being read or whose read failed
// keeps its row ("…" or "—"), so the hero is never lower than it looks without saying why.

import type { Balances } from "../api/generated/Balances";
import type { ListArrival } from "../components/emptyStates";

export type Pool = "core" | "shielded" | "platform";

/**
 * What Home knows of a Shielded or Platform balance: its duffs, or why there is no number.
 * `absent`: dw-app has no command for it or the network does not take it; `loading`: being read,
 * or not ready yet (a shielded account not scanned); `failed`: the read failed with none before it.
 */
export type PoolReading = number | "absent" | "loading" | "failed";

export interface HomeBalance {
  /** The hero: Core plus the pools whose balance is known, in duffs. */
  total: number;
  pools: { core: number; shielded: PoolReading; platform: PoolReading };
}

export const known = (r: PoolReading): r is number => typeof r === "number";

export const HERO_SYNC = {
  syncing: "Still syncing, balance may change",
  stopped: "Syncing has stopped, balance may be out of date",
  notStarted: "Not synced yet, balance may change",
  unreadable: "Could not check syncing, balance may change",
  unknownSyncing: "Syncing Balance",
  unknownNotSynced: "Not synced yet",
} as const;

/** A wallet this close to the tip while sync runs and the network reads caught up is between a
 * new block and its scan: the hero says nothing about it. */
export const HERO_NEAR_TIP_BLOCKS = 2;

export interface HeroSync {
  /** This wallet's catch-up, as its history reads it. */
  arrival: ListArrival;
  running: boolean;
  /** The status bar's caught-up reading, which holds through a new block (`settleSync`). */
  networkCaughtUp: boolean;
  /** The tip less this wallet's filter checkpoint; `null` while either is unknown. */
  blocksBehind: number | null;
  /** This wallet's first scan-height read is still out. */
  scanPending: boolean;
}

/** The line above the hero amount, from this wallet's own catch-up: empty once the wallet is
 * scanned to the tip, or within `HERO_NEAR_TIP_BLOCKS` of it during a new block; while it is not,
 * the balance may still move either way. */
export function heroSyncLine(balances: Balances | null, s: HeroSync): string {
  const settledBalance = balances !== null && !balances.catchingUp;
  if (settledBalance && s.running && s.networkCaughtUp) {
    if (s.scanPending) return "";
    if (s.blocksBehind !== null && s.blocksBehind <= HERO_NEAR_TIP_BLOCKS) return "";
  }
  const behind = s.arrival !== "settled" || balances?.catchingUp === true;
  if (!behind) return "";
  if (balances === null) return s.running ? HERO_SYNC.unknownSyncing : HERO_SYNC.unknownNotSynced;
  if (s.arrival === "unreadable") return HERO_SYNC.unreadable;
  if (s.running) return HERO_SYNC.syncing;
  return s.arrival === "starting" ? HERO_SYNC.notStarted : HERO_SYNC.stopped;
}

export function homeBalance(core: Balances, shielded: PoolReading, platform: PoolReading): HomeBalance {
  const pools = { core: core.confirmed + core.unconfirmed, shielded, platform };
  return { total: pools.core + (known(shielded) ? shielded : 0) + (known(platform) ? platform : 0), pools };
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
 * The breakdown's rows under the hero. Shielded and Platform show unless the wallet has no such
 * pool or it is known to be empty; one still being read, or whose read failed, keeps its row. Core
 * shows with them. Alone it would only repeat the hero, so there is no breakdown, and a line under
 * the hero names what is pending, immature or locked instead (`coreSubtitle`). In discreet mode
 * the rows don't depend on what holds money: Core and every pool the wallet has.
 */
export function breakdownRows(pools: HomeBalance["pools"], discreet: boolean): Pool[] {
  const others = (["shielded", "platform"] as const).filter((p) => pools[p] !== "absent" && (discreet || pools[p] !== 0));
  return others.length > 0 ? ["core", ...others] : [];
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
