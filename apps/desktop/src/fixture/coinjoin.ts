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
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { MixedCoinsChunk } from "../api/generated/MixedCoinsChunk";
import type { MixedCoinsConsolidationPlan } from "../api/generated/MixedCoinsConsolidationPlan";
import type { MixedCoinsResend } from "../api/generated/MixedCoinsResend";
import type { MixedCoinsSweepResult } from "../api/generated/MixedCoinsSweepResult";
import type { ThemeName } from "../api/generated/ThemeName";
import type { Utxo } from "../api/generated/Utxo";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";

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
  /** A scenario's failing "Move mixed coins" broadcast: chunk `at` (from 0) is rejected or of
   * unknown outcome, and the move stops there; unset, every chunk is sent. */
  sweepFailure?: { kind: "rejected" | "unknown"; at: number };
  /** Each wallet's last plan's quote: its id and its chunks' coins (`coinjoin.plan_changed` for another). */
  sweepQuote?: Record<string, { id: string; coins: string }>;
  /** The next move to Shielded meets coins outside its plan as it builds the lock: it spends its
   * grant and is refused `shielded.coins_changed`, with nothing tracked or sent. */
  drainChanged?: boolean;
  /** A move to Shielded whose lock signed and got no verdict: `next` spends the next move's grant and
   * refuses it `platform.broadcast_unknown`, then `stuck` refuses every later one the same before its
   * grant (the step reads possibly sent; settling it in Tools ▸ Repair is not modelled). */
  drainUnknown?: "next" | "stuck";
  /** The recorded chunks of unknown outcome, by wallet id: their display txids. */
  sweepUnknown?: Record<string, string[]>;
}

const NO_PROGRESS: CoinJoinProgress = { overallPercent: 0, denominatedPercent: 0, partiallyMixedPercent: 0, mixedPercent: 0, averageRounds: 0 };

export const newCoinJoinWorld = (): CoinJoinWorld => ({ settings: { ...COINJOIN_LIMITS.defaults }, mixers: {} });

const idleMixer = (): FixtureMixer => ({ state: "idle", stopReason: null, status: { kind: "idle" }, progress: NO_PROGRESS, sessions: [], queueSize: 0 });

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** The engine's answer to a sweep to Shielded: the move to Shielded is `shield_mixed_coins`. */
const notShielded = () => fail("platform.not_implemented", "not implemented: move_mixed_coins.shielded", { kind: "call", call: "move_mixed_coins.shielded" });

function unavailableOf(settings: CoinJoinSettings, w: FixtureWallet): CoinJoinStatus["unavailable"] {
  if (!settings.enabled) return { kind: "disabled" };
  if (w.summary.watchOnly) return { kind: "watchOnly" };
  const total = w.summary.balances?.confirmed ?? 0;
  if (total < COINJOIN_LIMITS.minMixingBalance) return { kind: "insufficientFunds", minDuffs: COINJOIN_LIMITS.minMixingBalance };
  return null;
}

/** `status_from_view` over the fixture wallet: its confirmed balance is what mixing can use; the
 * denominated and mixed parts are the mixer's percentages of it whatever the target (a lowered
 * target leaves them above the amount, as the engine's do). */
export function coinjoinStatusOf(world: World, w: FixtureWallet): CoinJoinStatus {
  const { settings } = world.coinjoin;
  const m = world.coinjoin.mixers[w.summary.walletId] ?? idleMixer();
  const total = w.summary.balances?.confirmed ?? 0;
  const target = settings.targetAmountDash * DUFFS_PER_DASH;
  const amount = Math.min(total, target);
  const fullyMixed = Math.floor((total * m.progress.mixedPercent) / 100);
  return {
    walletId: w.summary.walletId,
    state: m.state,
    stopReason: m.state === "idle" ? m.stopReason : null,
    unavailable: unavailableOf(settings, w),
    balances: {
      anonymizable: total - fullyMixed,
      denominated: Math.floor((total * m.progress.denominatedPercent) / 100),
      normalizedAnonymized: fullyMixed,
      fullyMixed,
      coinjoinAccount: w.coins.filter((u) => isCoinJoinCoin(u) && u.confirmations > 0).reduce((n, u) => n + u.amount, 0),
    },
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

/** dw-coinjoin `CoinJoinSettings::validate`, in its order and with its names; the goal's maximum
 * is the hard cap. */
const RANGES: [keyof CoinJoinSettings, string, number, (s: CoinJoinSettings) => number][] = [
  ["maxSessions", "max_sessions", COINJOIN_LIMITS.minSessions, () => COINJOIN_LIMITS.maxSessions],
  ["rounds", "rounds", COINJOIN_LIMITS.minRounds, () => COINJOIN_LIMITS.maxRounds],
  ["targetAmountDash", "target_amount_dash", COINJOIN_LIMITS.minAmountDash, () => COINJOIN_LIMITS.maxAmountDash],
  ["denomsHardCap", "denoms_hard_cap", COINJOIN_LIMITS.minDenoms, () => COINJOIN_LIMITS.maxDenoms],
  ["denomsGoal", "denoms_goal", COINJOIN_LIMITS.minDenoms, (s) => s.denomsHardCap],
];

// "Move mixed coins" (dw-engine coinjoin.rs `sweep_coins`, dw-coinjoin recovery.rs): the fixture's
// CoinJoin-account coins are its coins with mixing rounds; a chunk takes at most 500 of them above
// 1000 duffs, largest first, and pays 1 duff a byte (MIN_FEE_PER_KB).
const SWEEP_MIN_COIN = 1_000;
const SWEEP_MAX_INPUTS = 500;
const sweepSize = (inputs: number) => 8 + (inputs < 253 ? 1 : 3) + inputs * 148 + 1 + 34;

/** A plan's chunks' coins, in order, as one key. */
const chunkCoins = (chunks: { coins: Utxo[] }[]) => chunks.map((c) => c.coins.map((u) => `${u.outpoint.txid}:${u.outpoint.vout}`).join(",")).join("|");

/** A fresh display txid (64 hex digits) from the world's issue counter. */
function sweepTxid(w: World): string {
  w.issued += 1;
  return ("5eed" + w.issued.toString(16).padStart(4, "0")).repeat(8);
}

const isCoinJoinCoin = (u: Utxo) => u.coinjoinRounds !== null && u.amount > SWEEP_MIN_COIN && !u.reserved;

/** The CoinJoin account's coins (its addresses' coins): the fixture's coins with mixing rounds. */
export const coinJoinAccount = (w: FixtureWallet) => w.coins.filter((u) => u.coinjoinRounds !== null);

/** The wallet mixes or winds a session down (`coinjoin.mixing_active`, `shielded.mixing_active`). */
export const mixerBusy = (world: World, walletId: string) => (world.coinjoin.mixers[walletId]?.state ?? "idle") !== "idle";

/** The most coins one shield lock spends (`MAX_LOCK_INPUTS`): a consolidation pays for itself only above it. */
export const SHIELD_LOCK_INPUTS = 50;

/** Where a sweep's chunks go: a fresh wallet address ("Move mixed coins"), or the CoinJoin account itself
 * (the consolidation round before a move to Shielded). */
type SweepTarget = "wallet" | "coinjoin";

function sweepChunks(coins: Utxo[]): { coins: Utxo[]; chunk: MixedCoinsChunk }[] {
  const order = [...coins].sort((a, b) => b.amount - a.amount);
  const out: { coins: Utxo[]; chunk: MixedCoinsChunk }[] = [];
  for (let i = 0; i < order.length; i += SWEEP_MAX_INPUTS) {
    const part = order.slice(i, i + SWEEP_MAX_INPUTS);
    const total = part.reduce((n, u) => n + u.amount, 0);
    const fee = sweepSize(part.length);
    if (total > fee) out.push({ coins: part, chunk: { inputs: part.length, amount: total - fee, fee } });
  }
  return out;
}

interface CoinJoinDeps {
  wallet: (w: World, walletId: string) => FixtureWallet;
  grant: (w: World, id: string, walletId: string, kinds: GrantPurposeSpec["kind"][]) => FixtureGrant | null;
  useGrant: (w: World, id: string) => void;
}

/** `sweep_plan`'s refusals, then the wallet's CoinJoin-account coins in chunks. */
function sweepPlanOf(world: World, w: FixtureWallet, target: SweepTarget) {
  if (w.summary.watchOnly) throw fail("coinjoin.watch_only", "fixture: this wallet is watch-only");
  if (mixerBusy(world, w.summary.walletId)) throw fail("coinjoin.mixing_active", "stop mixing first");
  // Confirmed coins only (`sweep_coins`); the rest wait for a confirmation.
  const account = w.coins.filter(isCoinJoinCoin);
  const coins = account.filter((u) => u.confirmations > 0);
  const unconfirmed = account.length - coins.length;
  let chunks = sweepChunks(coins);
  // A consolidation pays for itself only when the drain needs it: more coins than one shield lock
  // takes, and fewer coins after it.
  const taken = chunks.reduce((n, c) => n + c.coins.length, 0);
  if (target === "coinjoin" && (taken <= SHIELD_LOCK_INPUTS || chunks.length >= taken)) chunks = [];
  if (chunks.length === 0) throw fail("coinjoin.nothing_to_move", "fixture: the CoinJoin account has nothing to sweep");
  const swept = chunks.flatMap((c) => c.coins);
  const fullyMixed = swept.filter((u) => (u.coinjoinRounds ?? 0) >= world.coinjoin.settings.rounds).reduce((n, u) => n + u.amount, 0);
  const total = swept.reduce((n, u) => n + u.amount, 0);
  const coinsAfter = coinJoinAccount(w).length - swept.length + chunks.length;
  return { chunks, total, fullyMixed, unconfirmed, coinsAfter };
}

/** A fresh plan's quote, replacing the wallet's last one for `target`. */
function issueQuote(world: World, walletId: string, target: SweepTarget, chunks: { coins: Utxo[] }[]): string {
  const quoteId = `quote-${(world.issued += 1)}`;
  (world.coinjoin.sweepQuote ??= {})[`${walletId}:${target}`] = { id: quoteId, coins: chunkCoins(chunks) };
  return quoteId;
}

/** The engine compares under the fence: the confirmed plan's coins, used once. */
function redeemQuote(world: World, walletId: string, target: SweepTarget, quoteId: string, chunks: { coins: Utxo[] }[]) {
  const key = `${walletId}:${target}`;
  const quoted = world.coinjoin.sweepQuote?.[key];
  if (quoted?.id !== quoteId) throw fail("coinjoin.plan_changed", "the coins to move changed since the plan");
  delete world.coinjoin.sweepQuote?.[key];
  if (quoted.coins !== chunkCoins(chunks)) throw fail("coinjoin.plan_changed", "the coins to move changed since the plan");
}

export function coinjoinHandlers({ wallet, grant, useGrant }: CoinJoinDeps): Handlers {
  /** Runs the plan `quoteId` confirmed to `target` under a `spend` grant of its total: each chunk sent
   * becomes one coin, outside the CoinJoin account for a move, inside it for a consolidation;
   * `sweepFailure` stops at its chunk as a rejected or unknown broadcast. */
  const sweep = (ctx: Ctx, walletId: string, target: SweepTarget, quoteId: string, grantId: string): MixedCoinsSweepResult => {
    const { world } = ctx;
    const w = wallet(world, walletId);
    const { chunks, total } = sweepPlanOf(world, w, target);
    redeemQuote(world, walletId, target, quoteId, chunks);
    const g = grant(world, grantId, walletId, ["spend"]);
    if (g?.grant.purpose.kind !== "spend" || g.grant.purpose.maxDuffs < total) throw fail("coinjoin.grant_invalid", "grant invalid");
    useGrant(world, grantId);
    const txidList: string[] = [];
    let moved = 0;
    let fee = 0;
    let failureCode: string | null = null;
    let unknownTxid: string | null = null;
    for (const [i, c] of chunks.entries()) {
      const txid = sweepTxid(world);
      const failure = world.coinjoin.sweepFailure;
      if (failure && i >= failure.at) {
        failureCode = failure.kind === "rejected" ? "coinjoin.broadcast_rejected" : "coinjoin.broadcast_unknown";
        if (failure.kind === "unknown") {
          unknownTxid = txid;
          (world.coinjoin.sweepUnknown ??= {})[walletId] = [...(world.coinjoin.sweepUnknown[walletId] ?? []), txid];
          for (const u of c.coins) u.reserved = true;
        }
        break;
      }
      const spent = new Set(c.coins);
      w.coins = w.coins.filter((u) => !spent.has(u));
      // A consolidated output stays in the CoinJoin account (rounds 0: not a denomination).
      const inAccount = target === "coinjoin";
      w.coins.push({ ...c.coins[0], outpoint: { txid, vout: 0 }, amount: c.chunk.amount, confirmations: 0, coinjoinDenominated: false, coinjoinRounds: inAccount ? 0 : null, label: null, reserved: false });
      const b = w.summary.balances;
      if (b) {
        const value = c.chunk.amount + c.chunk.fee;
        const coinjoin = Math.max(0, b.coinjoin - (inAccount ? c.chunk.fee : value));
        w.summary = { ...w.summary, balances: { ...b, confirmed: b.confirmed - value, unconfirmed: b.unconfirmed + c.chunk.amount, total: b.total - c.chunk.fee, coinjoin } };
      }
      txidList.push(txid);
      moved += c.chunk.amount + c.chunk.fee;
      fee += c.chunk.fee;
    }
    changed(ctx, walletId);
    ctx.emit({ kind: "balances", network: world.launch.network.dirName, walletId, balances: w.summary.balances });
    return { txids: txidList, moved, received: moved - fee, fee, remaining: total - moved, failureCode, unknownTxid };
  };

  return {
    mixed_coins_sweep_plan: ({ walletId, destination }, { world }) => {
      if (destination !== "wallet") throw notShielded();
      const { chunks, total, fullyMixed, unconfirmed } = sweepPlanOf(world, wallet(world, walletId), "wallet");
      const quoteId = issueQuote(world, walletId, "wallet", chunks);
      return { destination, total, chunks: chunks.map((c) => c.chunk), fullyMixed, notFullyMixed: total - fullyMixed, unconfirmed, quoteId };
    },
    mixed_coins_consolidation_plan: ({ walletId }, { world }): MixedCoinsConsolidationPlan => {
      const { chunks, total, fullyMixed, unconfirmed, coinsAfter } = sweepPlanOf(world, wallet(world, walletId), "coinjoin");
      const quoteId = issueQuote(world, walletId, "coinjoin", chunks);
      return { total, chunks: chunks.map((c) => c.chunk), fullyMixed, notFullyMixed: total - fullyMixed, coinsAfter, unconfirmed, quoteId };
    },
    // Each chunk pays one fresh address of the CoinJoin account: one unconfirmed coin of the account.
    consolidate_mixed_coins: ({ walletId, quoteId, grantId }, ctx): MixedCoinsSweepResult => sweep(ctx, walletId, "coinjoin", quoteId, grantId),
    move_mixed_coins: ({ walletId, destination, quoteId, grantId }, ctx): MixedCoinsSweepResult => {
      if (destination !== "wallet") throw notShielded();
      return sweep(ctx, walletId, "wallet", quoteId, grantId);
    },
    // The engine sends a recorded chunk's bytes again; the fixture's network never answers.
    resend_mixed_coins: ({ walletId, txid }, { world }): MixedCoinsResend[] => {
      wallet(world, walletId);
      const recorded = world.coinjoin.sweepUnknown?.[walletId] ?? [];
      if (txid !== null && !recorded.includes(txid)) return [{ txid, state: "resolved" }];
      return recorded.filter((t) => txid === null || t === txid).map((t) => ({ txid: t, state: "unknown" }));
    },
    coinjoin_limits: () => COINJOIN_LIMITS,
    coinjoin_settings: (_, { world }) => world.coinjoin.settings,
    // The engine's `invalid_argument` carries `SettingOutOfRange`'s text, naming the field.
    set_coinjoin_settings: ({ settings }, ctx) => {
      for (const [field, name, min, maxOf] of RANGES) {
        const v = settings[field] as number;
        const max = maxOf(settings);
        if (!Number.isInteger(v) || v < min || v > max) throw fail("invalid_argument", `${name} = ${v} is outside ${min}..=${max}`);
      }
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

/** `n` coins on the first wallet's CoinJoin account, counted in its balances: denominations of
 * 0.1 DASH, every third one short of the network's rounds (not fully mixed). With `split`, the
 * coins are mixed out of the wallet's first coin, so its totals stay as they are. */
function withMixedCoins(w: World, n: number, split = false): World {
  const main = w.wallets[0];
  const rounds = w.coinjoin.settings.rounds;
  const coins = Array.from({ length: n }, (_, i): Utxo => ({
    ...main.coins[0],
    outpoint: { txid: (0xc0 + (i % 0x30)).toString(16).repeat(32), vout: 100 + i },
    amount: 10_000_100,
    label: null,
    reserved: false,
    coinjoinDenominated: true,
    coinjoinRounds: i % 3 === 2 ? rounds - 1 : rounds,
  }));
  const sum = coins.reduce((s, u) => s + u.amount, 0);
  const added = split ? 0 : sum;
  main.coins = [...(split ? [{ ...main.coins[0], amount: main.coins[0].amount - sum }, ...main.coins.slice(1)] : main.coins), ...coins];
  const b = main.summary.balances!;
  main.summary = { ...main.summary, balances: { ...b, confirmed: b.confirmed + added, total: b.total + added, coinjoin: b.coinjoin + sum } };
  return w;
}

export function coinjoinScenarios(base: Base) {
  return {
    /** S7: 42 %, one session waiting in a masternode's queue. Mixed coins are on the CoinJoin
     * account, so the page offers "Move mixed coins", as a real wallet's does. */
    "coinjoin-mixing": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { status: { kind: "waitingInQueue" }, progress: PROGRESS_42, sessions: [SESSION], queueSize: 3 }), 3, true),
    /** S5: running, but no masternode list yet. */
    "coinjoin-paused": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { status: { kind: "noMasternodes" }, progress: PROGRESS_42 }), 3, true),
    /** S6: running and fully mixed: it waits for new coins. */
    "coinjoin-finished": (t: ThemeName | null) => mixing(base(t), { status: { kind: "notEnoughFunds" }, progress: PROGRESS_100 }),
    /** S3: Stop was pressed; the sessions are being reset. */
    "coinjoin-stopping": (t: ThemeName | null) => mixing(base(t), { state: "stopping", status: { kind: "mixingInProgress" }, progress: PROGRESS_42 }),
    /** S10: the vault locked while mixing (auto lock); unlocked again since. */
    "coinjoin-locked": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { state: "idle", stopReason: "vaultLocked", progress: PROGRESS_42 }), 3, true),
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
    /** "Move mixed coins": mixing stopped, with fully and partly mixed coins to move. */
    "coinjoin-move": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_42 }), 3),
    /** "Move mixed coins" while mixing runs: the sheet offers Stop Mixing. */
    "coinjoin-move-mixing": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { status: { kind: "waitingInQueue" }, progress: PROGRESS_42 }), 3),
    /** 600 coins in two chunks; the second is rejected: a partial move. */
    "coinjoin-move-partial": (t: ThemeName | null) => {
      const w = withMixedCoins(mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_42 }), 600);
      w.coinjoin.sweepFailure = { kind: "rejected", at: 1 };
      return w;
    },
    /** 60 coins: more than one move to Shielded takes, so they are combined first. */
    "coinjoin-move-many": (t: ThemeName | null) => withMixedCoins(mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_42 }), 60),
    /** "Move mixed coins" on a network without Shielded: the card offers the wallet only. */
    "coinjoin-move-no-shielded": (t: ThemeName | null) => {
      const w = withMixedCoins(mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_42 }), 3);
      w.pending.shielded = null;
      return w;
    },
    /** The only chunk's broadcast outcome is unknown. */
    "coinjoin-move-unknown": (t: ThemeName | null) => {
      const w = withMixedCoins(mixing(base(t), { state: "idle", stopReason: "userRequested", progress: PROGRESS_42 }), 3);
      w.coinjoin.sweepFailure = { kind: "unknown", at: 0 };
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
