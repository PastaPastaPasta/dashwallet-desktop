// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's unfinished transfers (engine `platform/unfinished.rs`): Core → Shielded locks a
// wallet restore read back from the chain, which Platform is not known to have applied. A scenario
// sets the locks, whether the list is complete, and how many finishes fail before one is applied.

import type { AppError } from "../api/generated/AppError";
import type { FinishReport } from "../api/generated/FinishReport";
import type { LockFinish } from "../api/generated/LockFinish";
import type { LockKind } from "../api/generated/LockKind";
import type { TransfersReport } from "../api/generated/TransfersReport";
import type { UnfinishedLock } from "../api/generated/UnfinishedLock";
import type { UnfinishedTransfers } from "../api/generated/UnfinishedTransfers";
import { SHIELDED_CODES } from "../api/pending";
import { checkGrant, useGrant, wallet, type Ctx } from "./handlers";
import type { World } from "./scenarios";

const POOL_FEE_CREDITS = 10_000_000n;
const CREDITS_PER_DUFF = 1000n;

export interface RecoveryWorld {
  locks: UnfinishedLock[];
  /** `false` while sync or a restore's scan has not caught up. */
  complete: boolean;
  /** The next finishes that end `platform.unavailable`. */
  failures: number;
  /** How many times a checking lock answers `checking` before a finish applies it; `null`: always. */
  checksBeforeApply: number | null;
  /** The finishes of a checking lock so far. */
  checks: number;
}

export const newRecoveryWorld = (): RecoveryWorld => ({ locks: [], complete: true, failures: 0, checksBeforeApply: null, checks: 0 });

/** A restored Core → Shielded lock for a scenario. */
export const restoredLock = (n: number, duffs: number, state: UnfinishedLock["state"] = "ready"): UnfinishedLock => ({
  txid: `${n.toString(16).padStart(2, "0")}5ec0de`.repeat(8).slice(0, 64),
  vout: 0,
  kind: "shielded",
  duffs,
  origin: "restored",
  state,
});

const raise = (code: string, message: string, data: unknown = null): never => {
  throw { code, message, data } as unknown as AppError;
};

const sameLock = (l: UnfinishedLock, kind: LockKind, txid: string, vout: number) => l.kind === kind && l.txid === txid && l.vout === vout;

/** The lock is applied: it leaves the list and its credits join the Shielded balance. */
function apply(world: World, r: RecoveryWorld, lock: UnfinishedLock) {
  r.locks = r.locks.filter((l) => l !== lock);
  const s = world.pending.shielded;
  if (s) s.balance = String(BigInt(s.balance) + BigInt(lock.duffs) * CREDITS_PER_DUFF - POOL_FEE_CREDITS);
}

function announce(ctx: Ctx, walletId: string) {
  ctx.emit({ kind: "platform", network: ctx.world.launch.network.dirName, walletId, change: { domain: "shielded" } });
}

/** `finish_asset_locks`' part, counted as the engine does: a ready lock is tried and applied, a checking
 * one is tried and stays, a confirming or not-active one is not tried and stays pending. `null`
 * while no lock is listed. */
export function finishRestored(ctx: Ctx, walletId: string): TransfersReport | null {
  const r = ctx.world.pending.recovery;
  if (r.locks.length === 0) return null;
  const listed = r.locks.length;
  const ready = r.locks.filter((l) => l.state === "ready");
  const tried = ready.length + r.locks.filter((l) => l.state === "checking").length;
  for (const lock of ready) apply(ctx.world, r, lock);
  if (tried > 0) announce(ctx, walletId);
  return { kind: "shielded", resumed: tried, completed: ready.length, stillPending: listed - ready.length };
}

export const RECOVERY_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  unfinished_transfers: ({ walletId }: { walletId: string }, ctx: Ctx): UnfinishedTransfers => {
    wallet(ctx.world, walletId);
    const r = ctx.world.pending.recovery;
    return { locks: r.locks, complete: r.complete };
  },
  finish_transfer: ({ walletId, kind, txid, vout, grant }: { walletId: string; kind: LockKind; txid: string; vout: number; grant: string }, ctx: Ctx): LockFinish => {
    wallet(ctx.world, walletId);
    const r = ctx.world.pending.recovery;
    const g = checkGrant(ctx.world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs < 1) return raise("platform.grant_invalid", "grant invalid");
    const lock = r.locks.find((l) => sameLock(l, kind, txid, vout));
    if (!lock) return raise("invalid_argument", "not an unfinished transfer", { kind: "reason", reason: "not an unfinished transfer" });
    useGrant(ctx.world, grant);
    if (lock.state === "confirming") return { kind: "proofWaiting" };
    if (lock.state === "notActive") return { kind: "notApplied", error: SHIELDED_CODES.notActive };
    if (lock.state === "checking") {
      r.checks++;
      if (r.checksBeforeApply === null || r.checks <= r.checksBeforeApply) return { kind: "checking" };
      apply(ctx.world, r, lock);
      announce(ctx, walletId);
      return { kind: "applied" };
    }
    if (lock.state !== "ready") return { kind: "checking" };
    if (r.failures > 0) {
      r.failures--;
      return { kind: "notApplied", error: "platform.unavailable" };
    }
    apply(ctx.world, r, lock);
    announce(ctx, walletId);
    return { kind: "applied" };
  },
};

/** Finish transfers' report: the registrations' counts and the restored locks' (`byKind` has a row for a kind with any listed lock). */
export const reportOf = (rows: { resumed: number; completed: number; stillPending: number }, restored: TransfersReport | null): FinishReport => ({
  resumed: rows.resumed + (restored?.resumed ?? 0),
  completed: rows.completed + (restored?.completed ?? 0),
  stillPending: rows.stillPending + (restored?.stillPending ?? 0),
  alreadyUsed: 0,
  byKind: restored ? [restored] : [],
  unread: [],
});
