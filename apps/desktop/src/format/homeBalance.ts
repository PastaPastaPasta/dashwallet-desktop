// Home's balance (0.1 release plan, the iOS model): the hero is everything the wallet can spend,
// the Core balance that can be spent (confirmed + unconfirmed, mixed coins included; immature and
// locked funds are not) plus the shielded one plus the Platform addresses'; the breakdown names the
// pools. Only a pool's known balance counts in the hero: one still being read or whose read failed
// keeps its row ("…" or "—"), so the hero is never lower than it looks without saying why.

import type { Balances } from "../api/generated/Balances";
import type { ShieldedHold } from "../api/queries";
import type { ListArrival } from "../components/emptyStates";
import { messageTable, t } from "../i18n";

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

export const HERO_SYNC = messageTable("home.sync", ["syncing", "stopped", "notStarted", "unreadable", "unknownSyncing", "unknownNotSynced"] as const);

/** How long, from this wallet's Home mounting, its first scan-height read may be out before the
 * hero says the balance may change. */
export const HERO_FIRST_READ_QUIET_MS = 2_000;
/** How long, from a tip change seen on a wallet that was at the tip, its scan may trail by up to
 * `HERO_NEW_BLOCK_BLOCKS` with the hero saying nothing. */
export const HERO_NEW_BLOCK_HOLD_MS = 30_000;
export const HERO_NEW_BLOCK_BLOCKS = 2;

/** One reading of this wallet's sync, as the hero needs it. */
export interface HeroReading {
  walletId: string;
  /** This wallet's catch-up, as its history reads it. */
  arrival: ListArrival;
  running: boolean;
  /** The status bar's caught-up reading. */
  networkCaughtUp: boolean;
  /** The engine's raw tip; 0 is unknown. */
  tip: number | null;
  /** This wallet's filter checkpoint; `undefined` until read. */
  scanHeight: number | null | undefined;
  /** No scan-height read has answered yet. */
  scanPending: boolean;
  /** The last sync-status or scan-height read failed, whatever is still cached. */
  readFailed: boolean;
}

/** What the hero remembers between readings of one wallet. */
export interface HeroWatch {
  walletId: string;
  mountedAt: number;
  /** The wallet was last seen scanned to this tip. */
  settled: { tip: number; height: number } | null;
  /** A new block after `settled`: the scan may trail it for a while. */
  hold: { since: number; tip: number; height: number } | null;
}

export interface HeroSync {
  arrival: ListArrival;
  running: boolean;
  readFailed: boolean;
  /** Scanned to a known tip, by this wallet's own checkpoint. */
  atTip: boolean;
  /** The first read or a new block: say nothing yet. */
  quiet: boolean;
}

const knownTip = (tip: number | null): number | null => (tip !== null && tip > 0 ? tip : null);

const atTip = (r: HeroReading): boolean => {
  const tip = knownTip(r.tip);
  return r.arrival === "settled" && !r.readFailed && !r.scanPending && tip !== null && r.scanHeight === tip;
};

export const newHeroWatch = (walletId: string, now: number = Date.now()): HeroWatch => ({ walletId, mountedAt: now, settled: null, hold: null });

/** The watch after `r`. A hold starts only on a forward tip change seen after the wallet was at the
 * tip, and ends at once on anything that is not the scan catching up with that block. */
export function watchHero(prev: HeroWatch, r: HeroReading, now: number = Date.now()): HeroWatch {
  if (prev.walletId !== r.walletId) return watchHero(newHeroWatch(r.walletId, now), r, now);
  const tip = knownTip(r.tip);
  const height = r.scanHeight ?? null;
  if (atTip(r)) return { ...prev, settled: { tip: tip!, height: height! }, hold: null };
  const ended = { ...prev, settled: null, hold: null };
  if (!r.running || r.readFailed || r.scanPending || tip === null || height === null || height > tip) return ended;
  if (tip - height > HERO_NEW_BLOCK_BLOCKS) return ended;
  const hold = prev.hold;
  if (hold) {
    if (tip < hold.tip || height < hold.height || now - hold.since >= HERO_NEW_BLOCK_HOLD_MS) return ended;
    return { ...prev, hold: { ...hold, tip } };
  }
  if (prev.settled && tip > prev.settled.tip && height >= prev.settled.height) return { ...prev, hold: { since: now, tip, height } };
  return ended;
}

/** Whether the hero says nothing yet: the first scan-height read for `HERO_FIRST_READ_QUIET_MS`
 * while sync runs and the network reads caught up, or a held new block. */
export function heroQuiet(w: HeroWatch, r: HeroReading, now: number = Date.now()): boolean {
  if (r.readFailed || !r.running) return false;
  if (r.scanPending) return r.networkCaughtUp && now - w.mountedAt < HERO_FIRST_READ_QUIET_MS;
  const tip = knownTip(r.tip);
  const height = r.scanHeight ?? null;
  if (!w.hold || tip === null || height === null) return false;
  const gap = tip - height;
  return gap >= 1 && gap <= HERO_NEW_BLOCK_BLOCKS && now - w.hold.since < HERO_NEW_BLOCK_HOLD_MS;
}

/** When the quiet of `w` runs out (ms), for a timer to read again; `null` if nothing is waiting. */
export function heroQuietEnds(w: HeroWatch, r: HeroReading): number | null {
  if (r.scanPending) return w.mountedAt + HERO_FIRST_READ_QUIET_MS;
  return w.hold ? w.hold.since + HERO_NEW_BLOCK_HOLD_MS : null;
}

export function heroSync(w: HeroWatch, r: HeroReading, now: number = Date.now()): HeroSync {
  return { arrival: r.arrival, running: r.running, readFailed: r.readFailed, atTip: atTip(r), quiet: heroQuiet(w, r, now) };
}

/** The line above the hero amount, from this wallet's own catch-up: empty once the wallet is
 * scanned to a known tip, or while it is quiet; otherwise the balance may still move either way.
 * A failed read says so before anything else. */
export function heroSyncLine(balances: Balances | null, s: HeroSync): string {
  if (balances === null) {
    if (!s.readFailed && (s.quiet || s.arrival === "settled")) return "";
    return s.running ? HERO_SYNC.unknownSyncing : HERO_SYNC.unknownNotSynced;
  }
  if (s.readFailed || s.arrival === "unreadable") return HERO_SYNC.unreadable;
  if (!balances.catchingUp && (s.quiet || (s.arrival === "settled" && s.atTip))) return "";
  if (s.running) return HERO_SYNC.syncing;
  return s.arrival === "starting" ? HERO_SYNC.notStarted : HERO_SYNC.stopped;
}

export function homeBalance(core: Balances, shielded: PoolReading, platform: PoolReading): HomeBalance {
  const pools = { core: core.confirmed + core.unconfirmed, shielded, platform };
  return { total: pools.core + (known(shielded) ? shielded : 0) + (known(platform) ? platform : 0), pools };
}

/**
 * The Core row's subtitle: what is still arriving (already in the row's number, and the coins the
 * engine withholds, which are not: a payment they come from may never confirm), then what is in
 * the wallet but not spendable (not in it): immature rewards and locked coins.
 */
export function coreSubtitle(core: Balances, fmt: (duffs: number) => string, fallback: string): string {
  const parts: string[] = [];
  const pending = core.unconfirmed + core.withheld;
  if (pending > 0) parts.push(t("home.core.pending", { amount: fmt(pending) }));
  if (core.immature > 0) parts.push(t("home.core.immature", { amount: fmt(core.immature) }));
  if (core.locked > 0) parts.push(t("home.core.locked", { amount: fmt(core.locked) }));
  return parts.length > 0 ? parts.join(t("home.core.separator")) : fallback;
}

/** The Shielded row's subtitle while payments that may still go through hold part of it. */
export const shieldedSubtitle = (hold: ShieldedHold | null, fmt: (duffs: number) => string, fallback: string): string =>
  hold ? t("home.shielded.held", { amount: fmt(hold.duffs), count: hold.count }) : fallback;

const poolRow = (pool: Pool) => ({
  pool,
  get title() {
    return t(`home.pools.${pool}.title`);
  },
  get subtitle() {
    return t(`home.pools.${pool}.subtitle`);
  },
});

export const POOL_ROWS: readonly { pool: Pool; readonly title: string; readonly subtitle: string }[] = [poolRow("core"), poolRow("shielded"), poolRow("platform")];

/**
 * The breakdown's rows under the hero. Shielded and Platform show unless the wallet has no such
 * pool or it is known to be empty; one still being read, or whose read failed, keeps its row. Core
 * shows with them. Alone it would only repeat the hero, so there is no breakdown, and a line under
 * the hero names what is pending, immature or locked instead (`coreSubtitle`). In discreet mode
 * the rows don't depend on what holds money: Core and every pool the wallet has. An empty Shielded
 * balance keeps its row while a payment holds part of it (`shieldedHeld`), to say so.
 */
export function breakdownRows(pools: HomeBalance["pools"], discreet: boolean, shieldedHeld = false): Pool[] {
  const others = (["shielded", "platform"] as const).filter((p) => pools[p] !== "absent" && (discreet || pools[p] !== 0 || (p === "shielded" && shieldedHeld)));
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
