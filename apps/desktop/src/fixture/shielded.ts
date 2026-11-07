// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's shielded pool (lane E1's contract, m4-dashpay-engine v41 §2.13): the account's
// status, receive address, the three moves (Core → Shielded, Shielded → Shielded, Shielded → Core)
// with their quotes and grants, and the activity rows. A scenario switches the network off
// (`active`), makes the next spend end unknown or refused, or the pool too small to withdraw from.

import type { AppError } from "../api/generated/AppError";
import type { PendingShield } from "../api/generated/PendingShield";
import type { ShieldedActivity } from "../api/generated/ShieldedActivity";
import type { ShieldedAddress } from "../api/generated/ShieldedAddress";
import type { ShieldedSpendOutcome } from "../api/generated/ShieldedSpendOutcome";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import type { ShieldedSpendState } from "../api/generated/ShieldedSpendState";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import type { UnrecordedSpend } from "../api/generated/UnrecordedSpend";
import type { UnresolvedSpend } from "../api/generated/UnresolvedSpend";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import type { ShieldQuote } from "../api/generated/ShieldQuote";
import type { MixedShieldQuote } from "../api/generated/MixedShieldQuote";
import type { Utxo } from "../api/generated/Utxo";
import { SHIELDED_CODES } from "../api/pending";
import { coinJoinAccount, mixerBusy, SHIELD_LOCK_INPUTS } from "./coinjoin";
import { checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import { FIXTURE_NOW } from "./wallet";

const CREDITS_PER_DUFF = 1000n;
/** Platform's flat fee for a shield, a shielded transfer and a withdrawal, in credits. */
const POOL_FEE_CREDITS = 10_000_000n;
const TRANSFER_FEE_CREDITS = 20_000_000n;
/** The lock's fee bound, in duffs. */
const LOCK_FEE_DUFFS = 1_000;
/** The smallest lock whose credits clear the pool fee. */
const MIN_SHIELD_DUFFS = 20_000;
const NOW = Math.floor(FIXTURE_NOW.getTime() / 1000);

/** What the next spend (a send, an unshield or a shield) comes to. */
export type SpendResult = "done" | "unknown" | "refused";

export interface ShieldedWorld {
  /** The network takes shielded transitions; `false`: every call but the status is `shielded.not_active`. */
  active: boolean;
  /** Keys bound and the notes scanned. */
  ready: boolean;
  proverReady: boolean;
  /** Spendable credits. */
  balance: string;
  address: string;
  pendingShields: PendingShield[];
  activity: ShieldedActivity[];
  next: SpendResult;
  /** Spends whose result is not confirmed (`pending`): the library re-sends them itself. */
  unresolved: UnresolvedSpend[];
  /** Spends of unknown result whose record was lost (DEC-1057). */
  unrecorded: UnrecordedSpend[];
  /** How a shield ends: applied, or the lock waiting for its proof. */
  shield: "applied" | "proofWaiting";
  /** The pool has too few notes to withdraw from (`shielded.pool_too_small`). */
  poolTooSmall: boolean;
}

export const newShieldedWorld = (): ShieldedWorld => ({
  active: true,
  ready: true,
  proverReady: true,
  balance: "125000000000",
  address: "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2",
  pendingShields: [],
  activity: [
    { id: "sh-3", kind: "received", direction: "in", credits: "50000000000", feeCredits: null, counterparty: null, memo: "Lunch", blockHeight: 1_200_100, status: "confirmed", createdAt: NOW - 2 * 86_400 },
    { id: "sh-2", kind: "sent", direction: "out", credits: "10000000000", feeCredits: "20000000", counterparty: "tdash1zxyz", memo: null, blockHeight: 1_200_050, status: "confirmed", createdAt: NOW - 4 * 86_400 },
    { id: "sh-1", kind: "shieldFromCore", direction: "selfTransfer", credits: "85000000000", feeCredits: "10000000", counterparty: null, memo: null, blockHeight: 1_200_000, status: "confirmed", createdAt: NOW - 6 * 86_400 },
  ],
  next: "done",
  unresolved: [],
  unrecorded: [],
  shield: "applied",
  poolTooSmall: false,
});

const raise = (code: string, message: string, data: unknown = null): never => {
  throw { code, message, data } as unknown as AppError;
};
const absent = (call: string) => ({ code: "platform.not_implemented", message: `the fixture backend has no handler for ${call}`, data: { kind: "call", call } });

function shieldedOf(ctx: Ctx, walletId: string, call: string, gated = true): ShieldedWorld {
  const s = ctx.world.pending.shielded;
  if (s === null) throw absent(call);
  wallet(ctx.world, walletId);
  if (gated && !s.active) raise(SHIELDED_CODES.notActive, "the fixture network takes no shielded transitions", { kind: "protocol", protocolVersion: 11, activationVersion: 12 });
  return s;
}

function status(s: ShieldedWorld): ShieldedStatus {
  if (!s.active) return { active: false, protocolVersion: 11, activationVersion: 12, ready: false, balance: "0", pending: "0", pendingShields: [], unresolvedSpends: [], unrecordedSpends: [], syncedIndex: null, lastSyncAt: null, syncing: false, proverReady: false };
  const pending = s.activity.filter((a) => a.status === "pending").reduce((n, a) => n + BigInt(a.credits), 0n);
  return { active: true, protocolVersion: 13, activationVersion: 12, ready: s.ready, balance: s.balance, pending: String(pending), pendingShields: s.pendingShields, unresolvedSpends: s.unresolved, unrecordedSpends: s.unrecorded, syncedIndex: s.ready ? 4096 : null, lastSyncAt: s.ready ? NOW - 20 : null, syncing: !s.ready, proverReady: s.proverReady };
}

const ready = (s: ShieldedWorld) => {
  if (!s.ready) raise(SHIELDED_CODES.notSynced, "Shielded is not synced yet");
};

/** The spend's grant: a `platformOp` whose credit budget covers `needed`. */
function spendGrant(ctx: Ctx, walletId: string, grant: string, needed: bigint) {
  const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
  if (g?.grant.purpose.kind !== "platformOp") return raise("platform.grant_invalid", "grant invalid");
  if (BigInt(g.grant.purpose.maxCredits) < needed) raise("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(needed), remaining: g.grant.purpose.maxCredits });
  useGrant(ctx.world, grant);
}

function addRow(s: ShieldedWorld, row: Omit<ShieldedActivity, "id" | "createdAt" | "blockHeight">) {
  s.activity = [{ ...row, id: `sh-${s.activity.length + 1}`, createdAt: NOW, blockHeight: row.status === "confirmed" ? 1_200_200 : null }, ...s.activity];
}

const grantFor = (credits: bigint) => ({ maxDuffs: 0, maxCredits: String(credits) });

function shieldQuote(ctx: Ctx, walletId: string, duffs: number): ShieldQuote {
  const s = shieldedOf(ctx, walletId, "shield_quote");
  // As the engine answers while the account is not established: the generic Platform refusal (pasta's preview, 03-…png).
  if (!s.ready) raise("platform.unavailable", "the fixture account is not established yet");
  if (duffs < MIN_SHIELD_DUFFS) raise(SHIELDED_CODES.belowMinimum, "below the minimum", { kind: "number", value: MIN_SHIELD_DUFFS });
  // A lock that may already be applied (`consumption_unknown`) blocks no new shield; any other pending one does.
  const blocking = s.pendingShields.find((p) => !p.consumptionUnknown);
  if (blocking) raise(SHIELDED_CODES.shieldPending, "an earlier shield is pending", { kind: "txid", txid: blocking.txid });
  const total = duffs + LOCK_FEE_DUFFS;
  return { duffs, feeDuffs: LOCK_FEE_DUFFS, totalDuffs: total, poolFeeCredits: String(POOL_FEE_CREDITS), credits: String(BigInt(duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS), grant: { maxDuffs: total, maxCredits: "0" } };
}

/** A drain lock's fee bound (`asset_lock_fee_bound`), in duffs. */
const drainFee = (inputs: number) => 1_000 + 148 * Math.min(inputs, SHIELD_LOCK_INPUTS);

/** "Move mixed coins" to Shielded (`shield_drain.rs`): the drain's checks, then `plan_of` over every
 * coin of the CoinJoin account, which the lock takes all of. */
function drainQuote(ctx: Ctx, walletId: string, call: string): { quote: MixedShieldQuote; coins: Utxo[] } {
  const s = shieldedOf(ctx, walletId, call);
  if (!s.ready) raise("platform.unavailable", "the fixture account is not established yet");
  if (mixerBusy(ctx.world, walletId)) raise(SHIELDED_CODES.mixingActive, "stop mixing first");
  const blocking = s.pendingShields.find((p) => !p.consumptionUnknown);
  if (blocking) raise(SHIELDED_CODES.shieldPending, "an earlier shield is pending", { kind: "txid", txid: blocking.txid });
  const coins = coinJoinAccount(wallet(ctx.world, walletId));
  if (coins.some((u) => u.reserved)) raise(SHIELDED_CODES.coinsLocked, "coins locked");
  const unconfirmed = coins.filter((u) => u.confirmations === 0).length;
  if (unconfirmed > 0) raise(SHIELDED_CODES.coinsUnconfirmed, `${unconfirmed} mixed coins are not confirmed yet`, { kind: "number", value: unconfirmed });
  if (coins.length === 0) raise(SHIELDED_CODES.nothingToMove, "fixture: the CoinJoin account has nothing to drain");
  if (coins.length > SHIELD_LOCK_INPUTS) raise(SHIELDED_CODES.tooManyInputs, "too many inputs", { kind: "number", value: coins.length });
  const totalDuffs = coins.reduce((n, u) => n + u.amount, 0);
  const fullyMixedDuffs = coins.filter((u) => (u.coinjoinRounds ?? 0) >= ctx.world.coinjoin.settings.rounds).reduce((n, u) => n + u.amount, 0);
  const feeDuffs = drainFee(coins.length);
  const duffs = totalDuffs - feeDuffs;
  const min = Number(POOL_FEE_CREDITS / CREDITS_PER_DUFF) + 1;
  if (duffs < min) raise(SHIELDED_CODES.belowMinimum, "below the minimum", { kind: "number", value: min });
  const credits = String(BigInt(duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS);
  const quote = { inputs: coins.length, totalDuffs, feeDuffs, duffs, poolFeeCredits: String(POOL_FEE_CREDITS), credits, fullyMixedDuffs, notFullyMixedDuffs: totalDuffs - fullyMixedDuffs };
  return { quote, coins };
}

function spendQuote(ctx: Ctx, walletId: string, credits: string, expected: boolean, call: string): ShieldedSpendQuote {
  const s = shieldedOf(ctx, walletId, call);
  ready(s);
  const c = BigInt(credits);
  if (c <= 0n) raise(SHIELDED_CODES.belowMinimum, "below the minimum", { kind: "number", value: 1 });
  const total = c + TRANSFER_FEE_CREDITS;
  if (total > BigInt(s.balance)) raise(SHIELDED_CODES.insufficientFunds, "insufficient funds", { kind: "credits", needed: String(total), available: s.balance });
  return { credits, feeCredits: String(TRANSFER_FEE_CREDITS), totalCredits: String(total), expectedDuffs: expected ? Number(c / CREDITS_PER_DUFF) : null, grant: grantFor(total) };
}

/** A spend's end: refused, `pending` (it went out and its result is not confirmed: the notes stay
 * reserved, the row stays `pending` and the wallet re-sends it itself), or done with the balance debited. */
function endSpend(s: ShieldedWorld, total: bigint, row: Omit<ShieldedActivity, "id" | "createdAt" | "blockHeight" | "status">): { state: ShieldedSpendState; unresolved: UnresolvedSpend | null } {
  if (s.next === "refused") raise("platform.unavailable", "the fixture refuses the spend");
  if (s.next === "unknown") {
    addRow(s, { ...row, status: "pending" });
    const unresolved = { activityId: s.activity[0].id, transition: `ab${String(s.activity.length).padStart(62, "0")}`, attempts: 1 };
    s.unresolved = [...s.unresolved, unresolved];
    return { state: "pending", unresolved };
  }
  s.balance = String(BigInt(s.balance) - total);
  addRow(s, { ...row, status: "confirmed" });
  return { state: "done", unresolved: null };
}

/** While the wallet holds an unrecorded spend, a send or unshield needs the user's confirmation (DEC-1057). */
function gateUnrecorded(ctx: Ctx, walletId: string, call: string, confirmed: boolean): void {
  const s = shieldedOf(ctx, walletId, call);
  if (s.unrecorded.length > 0 && confirmed !== true) raise(SHIELDED_CODES.unrecordedSpend, "a shielded payment from before the restart may still go through; confirm to send anyway");
}

function sendQuote(ctx: Ctx, walletId: string, to: string, credits: string): ShieldedSpendQuote {
  if (!/^t?dash1z/.test(to.trim())) {
    shieldedOf(ctx, walletId, "shielded_send_quote");
    raise(SHIELDED_CODES.invalidAddress, "not a shielded address", { kind: "reason", reason: "not a shielded address" });
  }
  return spendQuote(ctx, walletId, credits, false, "shielded_send_quote");
}

function shieldEnd(s: ShieldedWorld, txid: string, duffs: number): ShieldOutcome {
  const credits = BigInt(duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS;
  if (s.shield === "proofWaiting") {
    s.pendingShields = [...s.pendingShields.filter((p) => p.txid !== txid), { txid, duffs, proved: false, consumptionUnknown: false }];
    return { txid, state: { kind: "proofWaiting" } };
  }
  s.pendingShields = s.pendingShields.filter((p) => p.txid !== txid);
  s.balance = String(BigInt(s.balance) + credits);
  addRow(s, { kind: "shieldFromCore", direction: "selfTransfer", credits: String(credits), feeCredits: String(POOL_FEE_CREDITS), counterparty: null, memo: null, status: "confirmed" });
  return { txid, state: { kind: "applied", balance: s.balance } };
}

export const SHIELDED_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  shielded_status: ({ walletId }: { walletId: string }, ctx: Ctx) => status(shieldedOf(ctx, walletId, "shielded_status", false)),
  shielded_receive_address: ({ walletId }: { walletId: string }, ctx: Ctx): ShieldedAddress => {
    const s = shieldedOf(ctx, walletId, "shielded_receive_address");
    ready(s);
    return { address: s.address, account: 0 };
  },
  shield_quote: ({ walletId, duffs }: { walletId: string; duffs: number }, ctx: Ctx) => shieldQuote(ctx, walletId, duffs),
  shield_from_core: ({ walletId, duffs, grant }: { walletId: string; duffs: number; grant: string }, ctx: Ctx): ShieldOutcome => {
    const q = shieldQuote(ctx, walletId, duffs);
    const s = ctx.world.pending.shielded!;
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < q.totalDuffs) raise("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "duffs", needed: String(q.totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
    useGrant(ctx.world, grant);
    return shieldEnd(s, `lock${(s.activity.length + 1).toString(16).padStart(8, "0")}`, duffs);
  },
  resume_shield: ({ walletId, txid, grant }: { walletId: string; txid: string; grant: string }, ctx: Ctx): ShieldOutcome => {
    const s = shieldedOf(ctx, walletId, "resume_shield");
    const pending = s.pendingShields.find((p) => p.txid === txid);
    if (!pending) return raise("invalid_argument", "not a pending shield", { kind: "reason", reason: "not a pending shield" });
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs < 1) return raise("platform.grant_invalid", "grant invalid");
    useGrant(ctx.world, grant);
    s.shield = "applied";
    return shieldEnd(s, txid, pending.duffs);
  },
  shield_mixed_quote: ({ walletId }: { walletId: string }, ctx: Ctx): MixedShieldQuote => drainQuote(ctx, walletId, "shield_mixed_quote").quote,
  // The lock spends every coin of the CoinJoin account under a `spend` grant of their sum.
  shield_mixed_coins: ({ walletId, grant }: { walletId: string; grant: string }, ctx: Ctx): ShieldOutcome => {
    const { quote, coins } = drainQuote(ctx, walletId, "shield_mixed_coins");
    const g = checkGrant(ctx.world, grant, walletId, ["spend"]);
    if (g?.grant.purpose.kind !== "spend") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < quote.totalDuffs) raise("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "funding", needed: String(quote.totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
    useGrant(ctx.world, grant);
    if (ctx.world.coinjoin.drainChanged) {
      ctx.world.coinjoin.drainChanged = false;
      raise(SHIELDED_CODES.coinsChanged, "the mixed coins changed since the move was planned: review it again");
    }
    const w = wallet(ctx.world, walletId);
    const spent = new Set(coins);
    w.coins = w.coins.filter((u) => !spent.has(u));
    const b = w.summary.balances;
    if (b) w.summary = { ...w.summary, balances: { ...b, confirmed: b.confirmed - quote.totalDuffs, total: b.total - quote.totalDuffs, coinjoin: Math.max(0, b.coinjoin - quote.totalDuffs) } };
    const network = ctx.world.launch.network.dirName;
    ctx.emit({ kind: "coinJoin", network, walletId });
    ctx.emit({ kind: "balances", network, walletId, balances: w.summary.balances });
    const s = ctx.world.pending.shielded!;
    return shieldEnd(s, `drain${(s.activity.length + 1).toString(16).padStart(8, "0")}`, quote.duffs);
  },
  shielded_send_quote: ({ walletId, to, credits }: { walletId: string; to: string; credits: string }, ctx: Ctx) => {
    return sendQuote(ctx, walletId, to, credits);
  },
  shielded_send: ({ walletId, to, credits, memo, grant, confirmUnrecorded }: { walletId: string; to: string; credits: string; memo: string | null; grant: string; confirmUnrecorded: boolean }, ctx: Ctx): ShieldedSpendOutcome => {
    gateUnrecorded(ctx, walletId, "shielded_send", confirmUnrecorded);
    if (memo !== null && new TextEncoder().encode(memo).length > 36) raise("invalid_argument", "memo too long", { kind: "reason", reason: "memo over 36 bytes" });
    const q = sendQuote(ctx, walletId, to, credits);
    const s = ctx.world.pending.shielded!;
    spendGrant(ctx, walletId, grant, BigInt(q.totalCredits));
    const end = endSpend(s, BigInt(q.totalCredits), { kind: "sent", direction: "out", credits, feeCredits: q.feeCredits, counterparty: to, memo });
    return { credits, balance: s.balance, to, ...end };
  },
  unshield_quote: ({ walletId, credits }: { walletId: string; credits: string; to: string | null }, ctx: Ctx) => spendQuote(ctx, walletId, credits, true, "unshield_quote"),
  unshield_to_core: ({ walletId, credits, grant, confirmUnrecorded }: { walletId: string; credits: string; to: string | null; grant: string; confirmUnrecorded: boolean }, ctx: Ctx): ShieldedSpendOutcome => {
    gateUnrecorded(ctx, walletId, "unshield_to_core", confirmUnrecorded);
    const q = spendQuote(ctx, walletId, credits, true, "unshield_to_core");
    const s = ctx.world.pending.shielded!;
    if (s.poolTooSmall) raise(SHIELDED_CODES.poolTooSmall, "pool too small", { kind: "poolNotes", notes: 120, required: 250 });
    spendGrant(ctx, walletId, grant, BigInt(q.totalCredits));
    const to = HANDLERS.current_receive_address!({ walletId }, ctx).address;
    const end = endSpend(s, BigInt(q.totalCredits), { kind: "withdrawal", direction: "out", credits, feeCredits: q.feeCredits, counterparty: to, memo: null });
    return { credits, balance: s.balance, to, ...end };
  },
  shielded_history: ({ walletId, offset, limit }: { walletId: string; offset: number; limit: number }, ctx: Ctx) => {
    const s = shieldedOf(ctx, walletId, "shielded_history");
    if (limit > 500) raise("invalid_argument", "limit over 500", { kind: "reason", reason: "limit over 500" });
    return s.activity.slice(offset, offset + limit);
  },
};
