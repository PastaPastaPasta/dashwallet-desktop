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
import type { HeldSpend } from "../api/generated/HeldSpend";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import type { FundAmount } from "../api/generated/FundAmount";
import type { ShieldQuote } from "../api/generated/ShieldQuote";
import type { MixedShieldQuote } from "../api/generated/MixedShieldQuote";
import type { Utxo } from "../api/generated/Utxo";
import { SHIELDED_CODES } from "../api/pending";
import { coinJoinAccount, DRAIN_STEP, mixerBusy, SHIELD_LOCK_INPUTS } from "./coinjoin";
import { checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import type { PlatformWorld } from "./pending";
import type { FixtureWallet } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

const CREDITS_PER_DUFF = 1000n;
/** Platform's flat fee for a shield, a shielded transfer and a withdrawal, in credits. */
const POOL_FEE_CREDITS = 10_000_000n;
const TRANSFER_FEE_CREDITS = 20_000_000n;
/** An unshield to Platform's flat cost on top of the spend's fee (`compute_shielded_unshield_fee`): the
 * address it credits, 222 bytes at 27,000 + 400 credits a byte. */
export const UNSHIELD_ADDRESS_FEE_CREDITS = 222n * 27_400n;
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
  /** A sync pass is running on a ready account. */
  scanning?: boolean;
  /** When the last completed pass ended (Unix seconds); 20 s before the fixture's now if unset. */
  lastSyncAt?: number;
  /** The notes the balance is made of (credits): a spend is then priced per action, as the engine's
   * note selection prices it. Unset: one flat fee. */
  notes?: string[];
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
  /** Spends that may still go through, whose notes are held (out of `balance`). */
  held?: HeldSpend[];
  /** Credits the `held` spends hold. */
  heldCredits?: string;
  /** How a shield ends: applied, the lock waiting for its proof, proved with its apply unknown
   * (`checking`), or locked and refused by Platform (`notApplied`). */
  shield: "applied" | "proofWaiting" | "checking" | "notApplied";
  /** The pool has too few notes to withdraw from (`shielded.pool_too_small`). */
  poolTooSmall: boolean;
  /** The notes are resting until then (Unix seconds; C-funding F17): a registration cannot use
   * them yet. Unset: rested. */
  restingUntil?: number;
  /** A shield from the main balance whose lock signed and got no verdict (`SHIELD_STEP`): Tools ▸
   * Repair lists it, priced on every spendable coin of the main balance. */
  shieldUnknown?: boolean;
}

/** The step of the shield `shieldUnknown` leaves possibly sent. */
export const SHIELD_STEP = "shield/1/funding";

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

/** `n` payments that may still go through hold notes of `s`: 0.3 DASH each, sent an hour apart. */
export function holdShielded(s: ShieldedWorld, n: number): void {
  const held = Array.from({ length: n }, (_, i) => ({ activityId: `sh-held-${i + 1}`, transition: String(i + 1).repeat(64), credits: "30000000000" }));
  s.held = held;
  s.heldCredits = String(30_000_000_000n * BigInt(n));
  const rows: ShieldedActivity[] = held.map((h, i) => ({ id: h.activityId, kind: "sent", direction: "out", credits: "29980000000", feeCredits: "20000000", counterparty: "tdash1zxyz", memo: null, blockHeight: null, status: i === 0 ? "failed" : "unknown", createdAt: NOW - 3600 * (i + 1) }));
  s.activity = [...rows, ...s.activity];
}

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
  if (!s.active) return { active: false, protocolVersion: 11, activationVersion: 12, ready: false, balance: "0", pending: "0", pendingShields: [], unresolvedSpends: [], heldSpends: [], held: "0", unrecordedSpends: [], syncedIndex: null, lastSyncAt: null, syncing: false, proverReady: false };
  const pending = s.activity.filter((a) => a.status === "pending").reduce((n, a) => n + BigInt(a.credits), 0n);
  return { active: true, protocolVersion: 13, activationVersion: 12, ready: s.ready, balance: s.balance, pending: String(pending), pendingShields: s.pendingShields, unresolvedSpends: s.unresolved, heldSpends: s.held ?? [], held: s.heldCredits ?? "0", unrecordedSpends: s.unrecorded, syncedIndex: s.ready ? 4096 : null, lastSyncAt: s.ready ? (s.lastSyncAt ?? NOW - 20) : null, syncing: !s.ready || !!s.scanning, proverReady: s.proverReady };
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

/** How many of the main balance's coins a lock of `duffs` takes, largest first (the engine's
 * `fewest_inputs`): a lock takes at most `SHIELD_LOCK_INPUTS`. */
export function lockInputs(w: FixtureWallet, duffs: number): number {
  const values = w.coins.filter((u) => u.coinjoinRounds === null && u.spendable && !u.reserved).map((u) => u.amount).sort((a, b) => b - a);
  let sum = 0;
  for (let n = 0; n < values.length; n++) {
    if (sum >= duffs) return n;
    sum += values[n];
  }
  return values.length;
}

function shieldQuote(ctx: Ctx, walletId: string, duffs: number): ShieldQuote {
  const s = shieldedOf(ctx, walletId, "shield_quote");
  // As the engine answers while the account is not established: the generic Platform refusal (pasta's preview, 03-…png).
  if (!s.ready) raise("platform.unavailable", "the fixture account is not established yet");
  if (duffs < MIN_SHIELD_DUFFS) raise(SHIELDED_CODES.belowMinimum, "below the minimum", { kind: "number", value: MIN_SHIELD_DUFFS });
  // A lock that may already be applied (`consumption_unknown`) blocks no new shield; any other pending one does.
  const blocking = s.pendingShields.find((p) => !p.consumptionUnknown);
  if (blocking) raise(SHIELDED_CODES.shieldPending, "an earlier shield is pending", { kind: "txid", txid: blocking.txid });
  const total = duffs + LOCK_FEE_DUFFS;
  // As the engine: the main balance must cover the lock and its fee bound, in at most 50 coins.
  const w = wallet(ctx.world, walletId);
  const available = w.summary.balances?.confirmed ?? 0;
  if (total > available) raise(SHIELDED_CODES.fundingInsufficient, "insufficient funds", { kind: "duffs", needed: total, available });
  const inputs = lockInputs(w, total);
  if (inputs > SHIELD_LOCK_INPUTS) raise(SHIELDED_CODES.tooManyInputs, "too many inputs", { kind: "number", value: inputs });
  return { duffs, feeDuffs: LOCK_FEE_DUFFS, totalDuffs: total, poolFeeCredits: String(POOL_FEE_CREDITS), credits: String(BigInt(duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS), grant: { maxDuffs: total, maxCredits: "0" }, max: null };
}

/** A drain lock's fee bound (`asset_lock_fee_bound`), in duffs. */
const drainFee = (inputs: number) => 1_000 + 148 * Math.min(inputs, SHIELD_LOCK_INPUTS);

/** A Core Max's plan (`core_max.rs`): one lock of every final coin of the wallet's main account (not
 * a CoinJoin coin), the fee out of it, no change. The other accounts' coins stay (`leftDuffs`), as do
 * the main coins not final yet (`arrivingDuffs`). Refused as the engine refuses, each code under
 * `domain` (the shield's `shielded.*`, the fund's `credits.*`); below `floor` it names what Max would
 * move, what stays in the other accounts, what is still arriving and whether that plan of the final and arriving
 * coins together, every arriving coin taken as final, would clear `floor` (`arrivingClears`). */
export function coreMaxPlan(world: Ctx["world"], walletId: string, domain: "shielded" | "credits", floor: number) {
  if (mixerBusy(world, walletId)) raise(`${domain}.mixing_active`, "stop mixing first");
  const w = wallet(world, walletId);
  const main = w.coins.filter((u) => u.coinjoinRounds === null && !u.coinjoinDenominated && !w.otherAccounts?.includes(`${u.outpoint.txid}:${u.outpoint.vout}`));
  const final = main.filter((u) => u.confirmations > 0 || u.instantLocked);
  if (final.some((u) => u.userLocked || u.reserved)) raise(`${domain}.coins_locked`, "coins locked");
  if (final.length > SHIELD_LOCK_INPUTS) raise(`${domain}.too_many_inputs`, "too many inputs", { kind: "number", value: final.length });
  const totalDuffs = final.reduce((n, u) => n + u.amount, 0);
  const feeDuffs = drainFee(final.length);
  const duffs = Math.max(0, totalDuffs - feeDuffs);
  const leftDuffs = w.coins.filter((u) => !main.includes(u) && u.spendable && !u.reserved).reduce((n, u) => n + u.amount, 0);
  const arriving = main.filter((u) => !final.includes(u));
  const arrivingDuffs = arriving.reduce((n, u) => n + u.amount, 0);
  if (duffs < floor) {
    const all = [...final, ...arriving];
    const arrivingClears = arriving.length > 0 && !all.some((u) => u.userLocked || u.reserved) && all.length <= SHIELD_LOCK_INPUTS && totalDuffs + arrivingDuffs - drainFee(all.length) >= floor;
    raise(`${domain}.below_minimum`, "below the minimum", { kind: "maxMinimum", min: floor, duffs, leftDuffs, arrivingDuffs, arrivingClears });
  }
  return { totalDuffs, feeDuffs, duffs, max: { inputs: final.length, leftDuffs, arrivingDuffs } };
}

/** A shield's quote of `amount`: an amount with the fee on top, or a Core Max. */
function shieldQuoteOf(ctx: Ctx, walletId: string, amount: FundAmount): ShieldQuote {
  if (amount.kind === "duffs") return shieldQuote(ctx, walletId, amount.duffs);
  const s = shieldedOf(ctx, walletId, "shield_quote");
  if (!s.ready) raise("platform.unavailable", "the fixture account is not established yet");
  const blocking = s.pendingShields.find((p) => !p.consumptionUnknown);
  if (blocking) raise(SHIELDED_CODES.shieldPending, "an earlier shield is pending", { kind: "txid", txid: blocking.txid });
  const min = Number(POOL_FEE_CREDITS / CREDITS_PER_DUFF) + 1;
  const p = coreMaxPlan(ctx.world, walletId, "shielded", min);
  return { duffs: p.duffs, feeDuffs: p.feeDuffs, totalDuffs: p.totalDuffs, poolFeeCredits: String(POOL_FEE_CREDITS), credits: String(BigInt(p.duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS), grant: { maxDuffs: p.totalDuffs, maxCredits: "0" }, max: p.max };
}

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

/** A spend's fee per Orchard action, and the fewest actions a bundle has (`MIN_ACTIONS`). */
export const ACTION_FEE_CREDITS = 10_000_000n;
const MIN_ACTIONS = 2;
const actionsFee = (notes: number) => ACTION_FEE_CREDITS * BigInt(Math.max(notes, MIN_ACTIONS));

/** The fee of spending `amount` from `notes`, as rs-platform-wallet's `select_notes_with_fee` works it
 * out (note_selection.rs): largest notes first, from the fee of `MIN_ACTIONS`, re-selected with the
 * fee of the notes taken. A shortfall names `amount` plus the fee it had reached, not the fee of every
 * note. Throws `{ needed }`. */
export function noteSelectionFee(notes: readonly bigint[], amount: bigint, flat = 0n): bigint {
  const sorted = [...notes].sort((a, b) => (a < b ? 1 : a > b ? -1 : 0));
  const all = sorted.reduce((n, v) => n + v, 0n);
  const select = (fee: bigint) => {
    const required = amount + fee;
    if (all < required) throw { needed: required };
    let sum = 0n;
    let taken = 0;
    for (const v of sorted) {
      sum += v;
      taken++;
      if (sum >= required) break;
    }
    return { sum, taken };
  };
  let estimate = actionsFee(MIN_ACTIONS) + flat;
  for (let i = 0; i < 6; i++) {
    const { sum, taken } = select(estimate);
    const exact = actionsFee(taken) + flat;
    if (sum >= amount + exact) return exact;
    estimate = exact;
  }
  throw { needed: amount + estimate };
}

/** The notes of the balance: those set, and any credit added since as one more note. */
function notesOf(s: ShieldedWorld): bigint[] | null {
  if (!s.notes) return null;
  const notes = s.notes.map(BigInt);
  const extra = BigInt(s.balance) - notes.reduce((n, v) => n + v, 0n);
  return extra > 0n ? [...notes, extra] : notes;
}

function spendQuote(ctx: Ctx, walletId: string, credits: string, expected: boolean, call: string, flat = 0n): ShieldedSpendQuote {
  const s = shieldedOf(ctx, walletId, call);
  ready(s);
  const c = BigInt(credits);
  if (c <= 0n) raise(SHIELDED_CODES.belowMinimum, "below the minimum", { kind: "number", value: 1 });
  const short = (needed: bigint) => raise(SHIELDED_CODES.insufficientFunds, "insufficient funds", { kind: "credits", needed: String(needed), available: s.balance });
  const notes = notesOf(s);
  let fee = TRANSFER_FEE_CREDITS + flat;
  if (notes) {
    try {
      fee = noteSelectionFee(notes, c, flat);
    } catch (e) {
      short((e as { needed: bigint }).needed);
    }
  }
  const total = c + fee;
  if (total > BigInt(s.balance)) short(total);
  return { credits, feeCredits: String(fee), totalCredits: String(total), expectedDuffs: expected ? Number(c / CREDITS_PER_DUFF) : null, grant: grantFor(total) };
}

/** Holds back the notes a spend of `total` takes, largest first, as the engine reserves them while the
 * spend is unresolved: they leave the spendable balance, change and all. */
function reserve(s: ShieldedWorld, total: bigint) {
  const notes = notesOf(s);
  if (!notes) {
    s.balance = String(BigInt(s.balance) - total);
    return;
  }
  const left = [...notes].sort((a, b) => (a < b ? 1 : a > b ? -1 : 0));
  let held = 0n;
  while (held < total && left.length) held += left.shift()!;
  s.notes = left.map(String);
  s.balance = String(left.reduce((n, v) => n + v, 0n));
}

/** A spend's end: refused, `pending` (it went out and its result is not confirmed: the notes stay
 * reserved, the row stays `pending` and the wallet re-sends it itself), or done with the balance debited. */
function endSpend(s: ShieldedWorld, total: bigint, row: Omit<ShieldedActivity, "id" | "createdAt" | "blockHeight" | "status">): { state: ShieldedSpendState; unresolved: UnresolvedSpend | null } {
  if (s.next === "refused") raise("platform.unavailable", "the fixture refuses the spend");
  if (s.next === "unknown") {
    reserve(s, total);
    addRow(s, { ...row, status: "pending" });
    const unresolved = { activityId: s.activity[0].id, transition: `ab${String(s.activity.length).padStart(62, "0")}`, attempts: 1 };
    s.unresolved = [...s.unresolved, unresolved];
    return { state: "pending", unresolved };
  }
  s.balance = String(BigInt(s.balance) - total);
  if (s.notes) s.notes = s.balance === "0" ? [] : [s.balance];
  addRow(s, { ...row, status: "confirmed" });
  return { state: "done", unresolved: null };
}

/** The engine's flag for Shielded → Platform: off on mainnet. */
export const UNSHIELD_TO_PLATFORM = "unshield_to_platform";

function refuseOnMainnet(ctx: Ctx) {
  if (ctx.world.launch.network.kind === "mainnet") raise("platform.feature_off", UNSHIELD_TO_PLATFORM, { kind: "feature", feature: UNSHIELD_TO_PLATFORM });
}

/** The quote of an unshield to the wallet's own Platform address `to` (its receive address when `null`):
 * off on mainnet first, then the gates, then the own-address rule, then the spend's refusals. */
function toPlatformQuote(ctx: Ctx, walletId: string, credits: string, to: string | null, call: string): { quote: ShieldedSpendQuote; platform: PlatformWorld; address: string } {
  refuseOnMainnet(ctx);
  shieldedOf(ctx, walletId, call);
  const p = ctx.world.pending.platform;
  if (!p) return raise("platform.unavailable", "the fixture has no Platform addresses");
  const address = to?.trim() ?? p.receive.address;
  if (address !== p.receive.address && !p.addresses.some((a) => a.address === address)) raise(SHIELDED_CODES.invalidAddress, "not this wallet's Platform address", { kind: "reason", reason: "not this wallet's Platform address" });
  return { quote: spendQuote(ctx, walletId, credits, false, call, UNSHIELD_ADDRESS_FEE_CREDITS), platform: p, address };
}

function creditPlatform(p: PlatformWorld, address: string, credits: bigint) {
  const held = p.addresses.find((a) => a.address === address);
  if (held) held.credits = String(BigInt(held.credits) + credits);
  else p.addresses = [...p.addresses, { address, credits: String(credits) }];
  p.activity = [{ id: `unshield-${p.activity.length + 1}`, kind: "incoming", state: "done", credits: String(credits), address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: false, estimated: false }, ...p.activity];
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
  if (s.shield === "checking") {
    s.pendingShields = [...s.pendingShields.filter((p) => p.txid !== txid), { txid, duffs, proved: true, consumptionUnknown: true }];
    return { txid, state: { kind: "consumptionUnknown" } };
  }
  if (s.shield === "notApplied") {
    s.pendingShields = [...s.pendingShields.filter((p) => p.txid !== txid), { txid, duffs, proved: true, consumptionUnknown: false }];
    return { txid, state: { kind: "notApplied", error: "fixture: Platform refused the lock" } };
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
  shield_quote: ({ walletId, amount }: { walletId: string; amount: FundAmount }, ctx: Ctx) => shieldQuoteOf(ctx, walletId, amount),
  shield_from_core: ({ walletId, amount, grant }: { walletId: string; amount: FundAmount; grant: string }, ctx: Ctx): ShieldOutcome => {
    const q = shieldQuoteOf(ctx, walletId, amount);
    const duffs = q.duffs;
    const s = ctx.world.pending.shielded!;
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < q.totalDuffs) raise("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "duffs", needed: String(q.totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
    // A Core Max's grant is its quote's coins' sum: coins read again to another sum are not the quote's.
    if (amount.kind === "all" && g.grant.purpose.maxDuffs !== q.totalDuffs) raise(SHIELDED_CODES.coinsChanged, "the coins changed since the quote");
    useGrant(ctx.world, grant);
    // The engine's funding gate: a shield's asset lock waits while Core sync is not caught up.
    if (ctx.world.sync.running && !ctx.world.sync.caughtUp) raise(SHIELDED_CODES.notSynced, "Core sync is not caught up", { kind: "reason", reason: "core" });
    if (s.next === "refused") raise("platform.unavailable", "the fixture refuses the shield");
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
    const unknown = () => raise("platform.broadcast_unknown", "funding outcome unknown", { kind: "artifact", artifact: DRAIN_STEP });
    if (ctx.world.coinjoin.drainUnknown === "stuck") unknown();
    const g = checkGrant(ctx.world, grant, walletId, ["spend"]);
    if (g?.grant.purpose.kind !== "spend") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < quote.totalDuffs) raise("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "funding", needed: String(quote.totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
    useGrant(ctx.world, grant);
    if (ctx.world.coinjoin.drainChanged) {
      ctx.world.coinjoin.drainChanged = false;
      raise(SHIELDED_CODES.coinsChanged, "the mixed coins changed since the move was planned: review it again");
    }
    if (ctx.world.coinjoin.drainUnknown === "next") {
      ctx.world.coinjoin.drainUnknown = "stuck";
      unknown();
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
  unshield_to_platform_quote: ({ walletId, credits, to }: { walletId: string; credits: string; to: string | null }, ctx: Ctx) => toPlatformQuote(ctx, walletId, credits, to, "unshield_to_platform_quote").quote,
  unshield_to_platform: ({ walletId, credits, to, grant, confirmUnrecorded }: { walletId: string; credits: string; to: string | null; grant: string; confirmUnrecorded: boolean }, ctx: Ctx): ShieldedSpendOutcome => {
    refuseOnMainnet(ctx);
    gateUnrecorded(ctx, walletId, "unshield_to_platform", confirmUnrecorded);
    const { quote: q, platform: p, address } = toPlatformQuote(ctx, walletId, credits, to, "unshield_to_platform");
    const s = ctx.world.pending.shielded!;
    // As the engine: the grant is redeemed before Platform's pool floor refuses, so a floor refusal uses it up.
    spendGrant(ctx, walletId, grant, BigInt(q.totalCredits));
    if (s.poolTooSmall) raise(SHIELDED_CODES.poolTooSmall, "pool too small", { kind: "poolNotes", notes: 120, required: 250 });
    const end = endSpend(s, BigInt(q.totalCredits), { kind: "unshield", direction: "out", credits, feeCredits: q.feeCredits, counterparty: address, memo: null });
    // Until the From Shielded attribution lands, the address's rise reads as an incoming row.
    if (end.state === "done") creditPlatform(p, address, BigInt(credits));
    return { credits, balance: s.balance, to: address, ...end };
  },
  shielded_history: ({ walletId, offset, limit }: { walletId: string; offset: number; limit: number }, ctx: Ctx) => {
    const s = shieldedOf(ctx, walletId, "shielded_history");
    if (limit > 500) raise("invalid_argument", "limit over 500", { kind: "reason", reason: "limit over 500" });
    return s.activity.slice(offset, offset + limit);
  },
};
