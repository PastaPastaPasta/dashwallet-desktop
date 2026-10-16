// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's CoinJoin (U-06): the network's options with dw-coinjoin's ranges, and a mixer per
// wallet that starts, stops and halts under the engine's rules (dw-engine coinjoin.rs
// `start_mixing`, `stop_mixing`, `halt_mixing`, `status_from_view`). Mixing itself does not run:
// a scenario or a test sets the mixer's status and progress, and `emit`s `coinJoin` as the
// engine's refresher would.

import type { AppError } from "../api/generated/commands";
import type { CoinJoinLimits } from "../api/generated/CoinJoinLimits";
import type { CoinJoinProgress } from "../api/generated/CoinJoinProgress";
import type { CoinJoinSession } from "../api/generated/CoinJoinSession";
import type { CoinJoinSettings } from "../api/generated/CoinJoinSettings";
import type { CoinJoinState } from "../api/generated/CoinJoinState";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import type { CoinJoinStatusCode } from "../api/generated/CoinJoinStatusCode";
import type { CoinJoinStopReason } from "../api/generated/CoinJoinStopReason";
import type { ThemeName } from "../api/generated/ThemeName";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureWallet, World } from "./scenarios";

const DUFFS_PER_DASH = 100_000_000;

/** dw-coinjoin's constants (`denoms.rs`, `settings.rs`), as `coinjoin_limits` answers. */
export const COINJOIN_LIMITS: CoinJoinLimits = {
  denominations: [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001],
  minMixingBalance: 140_001,
  collateralAmount: 10_000,
  maxCollateralAmount: 40_000,
  minRounds: 2,
  maxRounds: 16,
  minSessions: 1,
  maxSessions: 10,
  minAmountDash: 2,
  maxAmountDash: 21_000_000,
  minDenoms: 10,
  maxDenoms: 100_000,
  defaults: { enabled: true, multiSession: false, maxSessions: 4, rounds: 4, targetAmountDash: 1000, denomsGoal: 50, denomsHardCap: 300 },
};

/** One wallet's mixer: what the engine keeps in memory (`Mixer`) and its last computed progress. */
export interface FixtureMixer {
  state: CoinJoinState;
  stopReason: CoinJoinStopReason | null;
  status: CoinJoinStatusCode;
  progress: CoinJoinProgress;
  sessions: CoinJoinSession[];
  queueSize: number;
}

export interface CoinJoinWorld {
  settings: CoinJoinSettings;
  /** By wallet id; a wallet not here has never mixed. */
  mixers: Record<string, FixtureMixer>;
}

const NO_PROGRESS: CoinJoinProgress = { overallPercent: 0, denominatedPercent: 0, partiallyMixedPercent: 0, mixedPercent: 0, averageRounds: 0 };

export const newCoinJoinWorld = (): CoinJoinWorld => ({ settings: { ...COINJOIN_LIMITS.defaults }, mixers: {} });

const idleMixer = (): FixtureMixer => ({ state: "idle", stopReason: null, status: { kind: "idle" }, progress: NO_PROGRESS, sessions: [], queueSize: 0 });

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** `status_from_view` over the fixture wallet: its confirmed balance is what mixing can use, the
 * mixed part follows the mixer's `mixedPercent` of the target amount. */
export function coinjoinStatusOf(world: World, w: FixtureWallet): CoinJoinStatus {
  const { settings } = world.coinjoin;
  const m = world.coinjoin.mixers[w.summary.walletId] ?? idleMixer();
  const total = w.summary.balances?.confirmed ?? 0;
  const target = settings.targetAmountDash * DUFFS_PER_DASH;
  const amount = Math.min(total, target);
  const fullyMixed = Math.floor((amount * m.progress.mixedPercent) / 100);
  const unavailable = !settings.enabled
    ? ({ kind: "disabled" } as const)
    : w.summary.watchOnly
      ? ({ kind: "watchOnly" } as const)
      : total < COINJOIN_LIMITS.minMixingBalance
        ? ({ kind: "insufficientFunds", minDuffs: COINJOIN_LIMITS.minMixingBalance } as const)
        : null;
  return {
    walletId: w.summary.walletId,
    state: m.state,
    stopReason: m.state === "idle" ? m.stopReason : null,
    unavailable,
    balances: { anonymizable: total - fullyMixed, denominated: Math.floor((amount * m.progress.denominatedPercent) / 100), normalizedAnonymized: fullyMixed, fullyMixed },
    progress: m.progress,
    amountAndRounds: { amount: amount === 0 ? target : amount, rounds: settings.rounds, insufficientInputs: total > 0 && total < target },
    submittedDenominations: m.sessions.flatMap((s) => (s.denomination === null ? [] : [s.denomination])),
    sessions: m.sessions,
    status: m.status,
    queueSize: m.queueSize,
    keysLeft: null,
  };
}

const changed = ({ world, emit }: Ctx, walletId: string) => emit({ kind: "coinJoin", network: world.launch.network.dirName, walletId });

/** `halt_mixing`: every mixing wallet stops and records why. */
export function haltAll(ctx: Ctx, reason: CoinJoinStopReason) {
  for (const [walletId, m] of Object.entries(ctx.world.coinjoin.mixers)) {
    if (m.state === "idle") continue;
    Object.assign(m, { state: "idle", stopReason: reason, status: { kind: "idle" }, sessions: [] });
    changed(ctx, walletId);
  }
}

const RANGES: [keyof CoinJoinSettings, number, number][] = [
  ["maxSessions", COINJOIN_LIMITS.minSessions, COINJOIN_LIMITS.maxSessions],
  ["rounds", COINJOIN_LIMITS.minRounds, COINJOIN_LIMITS.maxRounds],
  ["targetAmountDash", COINJOIN_LIMITS.minAmountDash, COINJOIN_LIMITS.maxAmountDash],
  ["denomsGoal", COINJOIN_LIMITS.minDenoms, COINJOIN_LIMITS.maxDenoms],
  ["denomsHardCap", COINJOIN_LIMITS.minDenoms, COINJOIN_LIMITS.maxDenoms],
];

export function coinjoinHandlers({ wallet }: { wallet: (w: World, walletId: string) => FixtureWallet }): Handlers {
  return {
    coinjoin_limits: () => COINJOIN_LIMITS,
    coinjoin_settings: (_, { world }) => world.coinjoin.settings,
    // dw-coinjoin `CoinJoinSettings::validate`: the message names the field.
    set_coinjoin_settings: ({ settings }, ctx) => {
      for (const [field, min, max] of RANGES) {
        const v = settings[field] as number;
        if (!Number.isInteger(v) || v < min || v > max) throw fail("invalid_argument", `coinjoin setting ${field} = ${v} outside ${min}..=${max}`);
      }
      if (settings.denomsGoal > settings.denomsHardCap) throw fail("invalid_argument", "coinjoin setting denomsGoal above denomsHardCap");
      ctx.world.coinjoin.settings = { ...settings };
      if (!settings.enabled) haltAll(ctx, "disabled");
      for (const w of ctx.world.wallets) changed(ctx, w.summary.walletId);
      return null;
    },
    coinjoin_status: ({ walletId }, { world }) => coinjoinStatusOf(world, wallet(world, walletId)),
    start_mixing: ({ walletId }, ctx) => {
      const { world } = ctx;
      const w = wallet(world, walletId);
      if (!world.coinjoin.settings.enabled) throw fail("coinjoin.disabled", "coinjoin is disabled");
      if (w.summary.watchOnly) throw fail("coinjoin.watch_only", "fixture: this wallet is watch-only");
      if (world.lock.state === "locked" || world.lock.state === "noVault") throw fail("coinjoin.vault_locked", "vault locked");
      const m = (world.coinjoin.mixers[walletId] ??= idleMixer());
      if (m.state === "mixing") return null;
      if ((w.summary.balances?.confirmed ?? 0) < COINJOIN_LIMITS.minMixingBalance)
        throw fail("coinjoin.insufficient_funds", "balance below the mixing minimum", { kind: "limit", duffs: COINJOIN_LIMITS.minMixingBalance });
      Object.assign(m, { state: "mixing", stopReason: null, status: { kind: "idle" } });
      changed(ctx, walletId);
      return null;
    },
    stop_mixing: ({ walletId }, ctx) => {
      wallet(ctx.world, walletId);
      const m = ctx.world.coinjoin.mixers[walletId];
      if (m && m.state !== "idle") {
        Object.assign(m, { state: "idle", stopReason: "userRequested", status: { kind: "idle" }, sessions: [] });
        changed(ctx, walletId);
      }
      return null;
    },
  };
}

// Scenarios (U-06 S-rows). The first wallet of `funded` with its mixer set; `funded` itself is
// S14 (never started) and `empty` S12.

const PROGRESS_42: CoinJoinProgress = { overallPercent: 42.7, denominatedPercent: 100, partiallyMixedPercent: 61.5, mixedPercent: 30.2, averageRounds: 1.8 };
const PROGRESS_100: CoinJoinProgress = { overallPercent: 100, denominatedPercent: 100, partiallyMixedPercent: 100, mixedPercent: 100, averageRounds: 4 };
const SESSION: CoinJoinSession = {
  proTxHash: "6a0f3b4cbd5e0e1f2a8b9c7d6e5f40312a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d",
  service: "34.211.10.5:19999",
  denomination: 100_001_000,
  state: "queue",
  entries: 0,
  lastMessage: null,
};

function mixing(w: World, m: Partial<FixtureMixer>): World {
  w.coinjoin.mixers[w.wallets[0].summary.walletId] = { ...idleMixer(), state: "mixing", ...m };
  return w;
}

type Base = (theme: ThemeName | null) => World;

export function coinjoinScenarios(base: Base) {
  return {
    /** S4: Start was pressed; the first step has not run. */
    "coinjoin-starting": (t: ThemeName | null) => mixing(base(t), {}),
    /** S7: 42 %, one session waiting in a masternode's queue. */
    "coinjoin-mixing": (t: ThemeName | null) => mixing(base(t), { status: { kind: "waitingInQueue" }, progress: PROGRESS_42, sessions: [SESSION], queueSize: 3 }),
    /** S5: running, but no masternode list yet. */
    "coinjoin-paused": (t: ThemeName | null) => mixing(base(t), { status: { kind: "noMasternodes" }, progress: PROGRESS_42 }),
    /** S6: running and fully mixed: it waits for new coins. */
    "coinjoin-finished": (t: ThemeName | null) => mixing(base(t), { status: { kind: "notEnoughFunds" }, progress: PROGRESS_100 }),
    /** S3: Stop was pressed; the sessions are being reset. */
    "coinjoin-stopping": (t: ThemeName | null) => mixing(base(t), { state: "stopping", status: { kind: "mixingInProgress" }, progress: PROGRESS_42 }),
    /** S10: the vault locked while mixing (auto lock); unlocked again since. */
    "coinjoin-locked": (t: ThemeName | null) => mixing(base(t), { state: "idle", stopReason: "vaultLocked", progress: PROGRESS_42 }),
    /** S11: the network session closed under the mixer. */
    "coinjoin-error": (t: ThemeName | null) => mixing(base(t), { state: "idle", stopReason: "sessionClosed", progress: PROGRESS_42 }),
    /** S8: CoinJoin features turned off in Settings. */
    "coinjoin-disabled": (t: ThemeName | null) => {
      const w = base(t);
      w.coinjoin.settings = { ...w.coinjoin.settings, enabled: false };
      return w;
    },
    /** S9: a watch-only wallet cannot mix. */
    "coinjoin-watch-only": (t: ThemeName | null) => {
      const w = base(t);
      w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
      return w;
    },
    /** Mixing with the vault unlocked for mixing only: sending stays locked. */
    "coinjoin-mixing-only": (t: ThemeName | null) => {
      const w = mixing(base(t), { status: { kind: "signing" }, progress: PROGRESS_42, sessions: [{ ...SESSION, state: "signing", entries: 3 }], queueSize: 2 });
      w.lock = { ...w.lock, state: "unlockedMixingOnly" };
      return w;
    },
    /** S13 with a custom level: 6 rounds set in Settings, mixing done and stopped. */
    "coinjoin-custom": (t: ThemeName | null) => {
      const w = mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_100 });
      w.coinjoin.settings = { ...w.coinjoin.settings, rounds: 6 };
      return w;
    },
  };
}
