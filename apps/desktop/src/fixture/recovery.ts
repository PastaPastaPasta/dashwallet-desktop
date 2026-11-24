// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's unfinished transfers (engine `platform/unfinished.rs`): Core → Shielded and "Add from
// Dash wallet" locks a wallet restore read back from the chain, which Platform is not known to have
// applied. A scenario sets the locks, whether the list is complete, and how many finishes fail before
// one is applied.

import type { AppError } from "../api/generated/AppError";
import type { FinishReport } from "../api/generated/FinishReport";
import type { LockFinish } from "../api/generated/LockFinish";
import type { LockKind } from "../api/generated/LockKind";
import type { RecoverQuote } from "../api/generated/RecoverQuote";
import type { TransfersReport } from "../api/generated/TransfersReport";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { SHIELDED_CODES } from "../api/pending";
import { checkGrant, useGrant, wallet, type Ctx } from "./handlers";
import type { PlatformWorld } from "./pending";
import type { World } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

const POOL_FEE_CREDITS = 10_000_000n;

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), as the fixture's cost table has it. */
export const CREDITS_PER_DUFF = 1000n;
/** Platform's fee for adding a lock, in credits, and the least a lock must carry to clear it. */
export const FUND_FEE_CREDITS = 56_000_000n;
export const FUND_MIN_DUFFS = 56_001;

export const fundCredits = (duffs: number) => BigInt(duffs) * CREDITS_PER_DUFF - FUND_FEE_CREDITS;

/** The lock `txid` is applied: it leaves the pending locks, the receive address gains the credits and
 * its `fromCore` row is `done` (recorded when the lock came from a restore and has none yet). */
export function applyFund(p: PlatformWorld, txid: string, credits: bigint) {
  p.funds = p.funds.filter((f) => f.txid !== txid);
  const held = p.addresses.find((a) => a.address === p.receive.address);
  if (held) held.credits = String(BigInt(held.credits) + credits);
  else p.addresses = [...p.addresses, { address: p.receive.address, credits: String(credits) }];
  const id = `fund-${txid}`;
  if (p.activity.some((a) => a.id === id)) p.activity = p.activity.map((a) => (a.id === id ? { ...a, state: "done" as const, credits: String(credits) } : a));
  else
    p.activity = [
      { id, kind: "fromCore", state: "done", credits: String(credits), address: p.receive.address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: false, estimated: false },
      ...p.activity,
    ];
}

export interface RecoveryWorld {
  locks: UnfinishedLock[];
  /** `false` while sync or a restore's scan has not caught up. */
  complete: boolean;
  /** The kinds whose side can't be read: none of their locks is listed or finished. */
  unread: LockKind[];
  /** The next finishes that end `platform.unavailable`. */
  failures: number;
  /** How many times a checking lock answers `checking` before a finish applies it; `null`: always. */
  checksBeforeApply: number | null;
  /** The finishes of a checking lock so far. */
  checks: number;
  /** The txids of listed locks Platform already took: their apply is answered already used. */
  consumed: string[];
}

export const newRecoveryWorld = (): RecoveryWorld => ({ locks: [], complete: true, unread: [], failures: 0, checksBeforeApply: null, checks: 0, consumed: [] });

/** A restored Core → Shielded lock for a scenario. */
export const restoredLock = (n: number, duffs: number, state: UnfinishedLock["state"] = "ready"): UnfinishedLock => ({
  txid: `${n.toString(16).padStart(2, "0")}5ec0de`.repeat(8).slice(0, 64),
  vout: 0,
  kind: "shielded",
  duffs,
  origin: "restored",
  state,
  remainingCredits: null,
});

/** A transfer to Shielded of this device's that Platform refused after a penalty, `credits` left
 * (recoverable at or above the fund fee, else too small). */
export const partlyUsedLock = (n: number, duffs: number, credits: bigint): UnfinishedLock => ({
  ...restoredLock(n, duffs, credits >= FUND_FEE_CREDITS ? "recoverable" : "tooSmall"),
  origin: "thisDevice",
  remainingCredits: String(credits),
});

/** A restored "Add from Dash wallet" lock: one no fund step of this install signed (a seed restored on a new install, or another device's). */
export const restoredFundLock = (n: number, duffs: number, state: UnfinishedLock["state"] = "ready"): UnfinishedLock => ({
  ...restoredLock(n, duffs, state),
  txid: `${n.toString(16).padStart(2, "0")}fa11ed`.repeat(8).slice(0, 64),
  kind: "platformAddress",
});

const raise = (code: string, message: string, data: unknown = null): never => {
  throw { code, message, data } as unknown as AppError;
};

const sameLock = (l: UnfinishedLock, kind: LockKind, txid: string, vout: number) => l.kind === kind && l.txid === txid && l.vout === vout;

/** The lock is applied: it leaves the list and its credits join the Shielded balance, or the wallet's
 * Platform receive address (the lock's `fromCore` row is done). */
function apply(world: World, r: RecoveryWorld, lock: UnfinishedLock) {
  r.locks = r.locks.filter((l) => l !== lock);
  if (lock.kind === "platformAddress") {
    if (world.pending.platform) applyFund(world.pending.platform, lock.txid, fundCredits(lock.duffs));
    return;
  }
  const s = world.pending.shielded;
  if (s) s.balance = String(BigInt(s.balance) + BigInt(lock.duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS);
}

function announce(ctx: Ctx, walletId: string, kind: LockKind) {
  ctx.emit({ kind: "platform", network: ctx.world.launch.network.dirName, walletId, change: { domain: kind === "platformAddress" ? "platformAddresses" : "shielded" } });
}

/** A lock's apply past the lease (engine `run`): one Platform already took is reported used and leaves
 * the list; a checking one answers checking until `checksBeforeApply` finishes; any other is applied. */
function tryLock(world: World, r: RecoveryWorld, lock: UnfinishedLock): LockFinish {
  if (r.consumed.includes(lock.txid)) {
    r.locks = r.locks.filter((l) => l !== lock);
    return { kind: "reportedUsed" };
  }
  if (lock.state === "checking") {
    r.checks++;
    if (r.checksBeforeApply === null || r.checks <= r.checksBeforeApply) return { kind: "checking" };
  }
  apply(world, r, lock);
  return { kind: "applied" };
}

/** `finish_asset_locks`' part, counted as the engine does, one row per kind that lists a lock: a ready
 * lock and a restored checking one are tried (one Platform already took counts in `alreadyUsed`); this
 * install's checking lock and a confirming, not-active, unsupported or too-small one are not tried and
 * stay pending (a checking one's own Check again tries it). */
export function finishRestored(ctx: Ctx, walletId: string): RestoredReport {
  const r = ctx.world.pending.recovery;
  const kinds: LockKind[] = ["shielded", "platformAddress"];
  let alreadyUsed = 0;
  const byKind = kinds.flatMap((kind) => {
    if (r.unread.includes(kind)) return [];
    const listed = r.locks.filter((l) => l.kind === kind);
    if (listed.length === 0) return [];
    const row: TransfersReport = { kind, resumed: 0, completed: 0, stillPending: 0 };
    for (const lock of listed) {
      if (lock.state !== "ready" && !(lock.state === "checking" && lock.origin === "restored")) {
        row.stillPending++;
        continue;
      }
      const got = tryLock(ctx.world, r, lock);
      row.resumed++;
      if (got.kind === "applied") row.completed++;
      else if (got.kind === "reportedUsed") alreadyUsed++;
      else row.stillPending++;
    }
    if (row.completed > 0) announce(ctx, walletId, kind);
    return [row];
  });
  return { byKind, alreadyUsed };
}

/** The restored locks' part of Finish transfers. */
export interface RestoredReport {
  byKind: TransfersReport[];
  alreadyUsed: number;
}

export const RECOVERY_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  unfinished_transfers: ({ walletId }: { walletId: string }, ctx: Ctx): UnfinishedTransfers => {
    wallet(ctx.world, walletId);
    const r = ctx.world.pending.recovery;
    return { locks: r.locks.filter((l) => !r.unread.includes(l.kind)), complete: r.complete, unread: r.unread };
  },
  finish_transfer: ({ walletId, kind, txid, vout, grant }: { walletId: string; kind: LockKind; txid: string; vout: number; grant: string }, ctx: Ctx): LockFinish => {
    wallet(ctx.world, walletId);
    const r = ctx.world.pending.recovery;
    const lock = r.locks.find((l) => sameLock(l, kind, txid, vout));
    if (!lock) return raise("invalid_argument", "not an unfinished transfer", { kind: "reason", reason: "not an unfinished transfer" });
    // The refusals the engine reads before the lease: the grant is not read, so it is kept.
    if (lock.state === "confirming") return { kind: "proofWaiting" };
    if (lock.state === "notActive") return { kind: "notApplied", error: SHIELDED_CODES.notActive };
    if (lock.state === "unsupported") return { kind: "notApplied", error: "recovery.unsupported_output" };
    if (lock.state === "tooSmall") return { kind: "notApplied", error: lock.kind === "platformAddress" ? "credits.below_minimum" : "shielded.below_minimum" };
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < 1) return raise("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "funding" });
    useGrant(ctx.world, grant);
    if (lock.state !== "ready" && lock.state !== "checking") return { kind: "checking" };
    if (lock.state === "ready" && r.failures > 0) {
      r.failures--;
      return { kind: "notApplied", error: "platform.unavailable" };
    }
    const got = tryLock(ctx.world, r, lock);
    if (got.kind === "applied") announce(ctx, walletId, lock.kind);
    return got;
  },
};

/** The lock is recovered: it leaves the list, the receive address gains what was left less the fund
 * fee, and a done `recovered` row joins the Platform activity. */
function recoverLock(world: World, r: RecoveryWorld, lock: UnfinishedLock, credits: bigint) {
  r.locks = r.locks.filter((l) => l !== lock);
  const p = world.pending.platform;
  if (!p) return;
  const gained = credits - FUND_FEE_CREDITS;
  const held = p.addresses.find((a) => a.address === p.receive.address);
  if (held) held.credits = String(BigInt(held.credits) + gained);
  else p.addresses = [...p.addresses, { address: p.receive.address, credits: String(gained) }];
  p.activity = [
    { id: `fund-${lock.txid}`, kind: "recovered", state: "done", credits: String(gained), address: p.receive.address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: false, estimated: false },
    ...p.activity,
  ];
}

const partlyUsedAt = (r: RecoveryWorld, txid: string, vout: number) => r.locks.find((l) => sameLock(l, "shielded", txid, vout) && l.remainingCredits !== null);

export const REMAINDER_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  recover_transfer_quote: ({ walletId, txid, vout }: { walletId: string; txid: string; vout: number }, ctx: Ctx): RecoverQuote => {
    wallet(ctx.world, walletId);
    const lock = partlyUsedAt(ctx.world.pending.recovery, txid, vout);
    if (!lock?.remainingCredits) return { kind: "notApplied", error: "recovery.not_recoverable" };
    const credits = lock.remainingCredits;
    const feeCredits = String(FUND_FEE_CREDITS);
    return BigInt(credits) >= FUND_FEE_CREDITS ? { kind: "ready", credits, feeCredits } : { kind: "tooSmall", credits, feeCredits };
  },
  recover_transfer: ({ walletId, txid, vout, grant }: { walletId: string; txid: string; vout: number; grant: string }, ctx: Ctx): LockFinish => {
    wallet(ctx.world, walletId);
    const r = ctx.world.pending.recovery;
    const lock = partlyUsedAt(r, txid, vout);
    if (!lock?.remainingCredits) return { kind: "notApplied", error: "recovery.not_recoverable" };
    const credits = BigInt(lock.remainingCredits);
    if (credits < FUND_FEE_CREDITS) return { kind: "notApplied", error: "credits.below_minimum" };
    // The engine's checks before redeeming: a funding budget, and exactly the credits left.
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") return raise("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < 1) return raise("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "funding" });
    if (BigInt(g.grant.purpose.maxCredits) !== credits) return raise("platform.grant_invalid", "grant invalid");
    useGrant(ctx.world, grant);
    if (r.failures > 0) {
      r.failures--;
      return { kind: "checking" };
    }
    recoverLock(ctx.world, r, lock, credits);
    announce(ctx, walletId, "platformAddress");
    return { kind: "applied" };
  },
};

/** Finish transfers' report: the registrations' counts and the restored locks' (`byKind` has a row for a kind with any listed lock). */
export const reportOf = (rows: { resumed: number; completed: number; stillPending: number }, restored: RestoredReport, unread: LockKind[] = []): FinishReport => ({
  resumed: rows.resumed + restored.byKind.reduce((n, k) => n + k.resumed, 0),
  completed: rows.completed + restored.byKind.reduce((n, k) => n + k.completed, 0),
  stillPending: rows.stillPending + restored.byKind.reduce((n, k) => n + k.stillPending, 0),
  alreadyUsed: restored.alreadyUsed,
  byKind: restored.byKind,
  unread,
});
