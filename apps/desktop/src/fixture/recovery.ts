// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's unfinished transfers (engine `platform/unfinished.rs`): Core → Shielded and "Add from
// Dash wallet" locks a wallet restore read back from the chain, which Platform is not known to have
// applied. A scenario sets the locks, whether the list is complete, and how many finishes fail before
// one is applied.

import type { AppError } from "../api/generated/AppError";
import type { FinishReport } from "../api/generated/FinishReport";
import type { LockFinish } from "../api/generated/LockFinish";
import type { LockKind } from "../api/generated/LockKind";
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
}

export const newRecoveryWorld = (): RecoveryWorld => ({ locks: [], complete: true, unread: [], failures: 0, checksBeforeApply: null, checks: 0 });

/** A restored Core → Shielded lock for a scenario. */
export const restoredLock = (n: number, duffs: number, state: UnfinishedLock["state"] = "ready"): UnfinishedLock => ({
  txid: `${n.toString(16).padStart(2, "0")}5ec0de`.repeat(8).slice(0, 64),
  vout: 0,
  kind: "shielded",
  duffs,
  origin: "restored",
  state,
});

/** A restored "Add from Dash wallet" lock (a seed restore, another device's, or a wallet imported again). */
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

/** `finish_asset_locks`' part, counted as the engine does, one row per kind that lists a lock: a ready
 * lock is tried and applied; a checking, confirming, not-active, unsupported or too-small one is not
 * tried and stays pending (a checking one's own Check again tries it). */
export function finishRestored(ctx: Ctx, walletId: string): TransfersReport[] {
  const r = ctx.world.pending.recovery;
  const kinds: LockKind[] = ["shielded", "platformAddress"];
  return kinds.flatMap((kind) => {
    if (r.unread.includes(kind)) return [];
    const listed = r.locks.filter((l) => l.kind === kind);
    if (listed.length === 0) return [];
    const ready = listed.filter((l) => l.state === "ready");
    for (const lock of ready) apply(ctx.world, r, lock);
    if (ready.length > 0) announce(ctx, walletId, kind);
    return [{ kind, resumed: ready.length, completed: ready.length, stillPending: listed.length - ready.length }];
  });
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
    if (lock.state === "checking") {
      r.checks++;
      if (r.checksBeforeApply === null || r.checks <= r.checksBeforeApply) return { kind: "checking" };
      apply(ctx.world, r, lock);
      announce(ctx, walletId, lock.kind);
      return { kind: "applied" };
    }
    if (lock.state !== "ready") return { kind: "checking" };
    if (r.failures > 0) {
      r.failures--;
      return { kind: "notApplied", error: "platform.unavailable" };
    }
    apply(ctx.world, r, lock);
    announce(ctx, walletId, lock.kind);
    return { kind: "applied" };
  },
};

/** Finish transfers' report: the registrations' counts and the restored locks' (`byKind` has a row for a kind with any listed lock). */
export const reportOf = (rows: { resumed: number; completed: number; stillPending: number }, restored: TransfersReport[], unread: LockKind[] = []): FinishReport => ({
  resumed: rows.resumed + restored.reduce((n, k) => n + k.resumed, 0),
  completed: rows.completed + restored.reduce((n, k) => n + k.completed, 0),
  stillPending: rows.stillPending + restored.reduce((n, k) => n + k.stillPending, 0),
  alreadyUsed: 0,
  byKind: restored,
  unread,
});
