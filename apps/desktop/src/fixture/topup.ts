// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// DP1-10's engine reads for the fixture: the balance refresh, the own contest's status and the
// credits top-up (DP1-06's `top_up_quote`, `top_up`, `pending_top_ups`, `resume_top_up`,
// `resolve_top_up`; DP1-10's `stuck_top_up_fundings`), each refusing in the engine's order
// (m4 §2.8): the quote's (the amount, a pending lock, the wallet's coins), then for `top_up` a
// stuck funding of the identity and the grant. DP6-02's withdrawals (`withdraw_quote`, `withdraw`,
// `withdrawals`) share the credits state: the rules of the newest protocol version, Max keeping
// the fee in reserve.

import type { AppError } from "../api/generated/commands";
import type { ContestStatus } from "../api/generated/ContestStatus";
import type { CostTable } from "../api/generated/CostTable";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { StuckTopUpFunding } from "../api/generated/StuckTopUpFunding";
import type { TopUpOutcome } from "../api/generated/TopUpOutcome";
import type { TopUpState } from "../api/generated/TopUpState";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import type { Withdrawal } from "../api/generated/Withdrawal";
import type { WithdrawOutcome } from "../api/generated/WithdrawOutcome";
import { FIXTURE_NOW } from "./wallet";
import { classifyAddress } from "./send";
import { IDENTITY, type DashPayWorld } from "./dashpay";
import type { Ctx, Handlers } from "./handlers";
import { fundable } from "./registration";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";

/** A wallet's credits state: its top-up locks Platform has not taken, its fundings whose outcome
 * is unknown, the own contest as `contest_status` reads it (`null`: derived from the status), and
 * what the next `top_up` comes to. */
export interface CreditsWorld {
  pending: PendingTopUp[];
  stuck: StuckTopUpFunding[];
  contest: ContestStatus | null;
  /** `broadcastUnknown`: the next funding's outcome is unknown, so its step is left stuck. */
  next: TopUpState["kind"] | "broadcastUnknown";
  /** Locks funded so far: each gets its own txid. */
  locks: number;
  /** DP6-02: Platform's withdrawal documents of the wallet's identities, newest first. */
  withdrawals: Withdrawal[];
  /** What the next `withdraw` comes to: Platform's answer, an unanswered broadcast whose document
   * is not found (`unknown`), or one whose document is (`landed`). */
  nextWithdraw: "applied" | "unknown" | "landed";
}

export const newCreditsWorld = (patch: Partial<CreditsWorld> = {}): CreditsWorld => ({
  pending: [],
  stuck: [],
  contest: null,
  next: "applied",
  locks: 0,
  withdrawals: [],
  nextWithdraw: "applied",
  ...patch,
});

/** Platform's withdrawal rules at the newest protocol version (credits): the reserve on Max (dpp's
 * minimum fee), the range, and the Core fee a payout carries (duffs). */
export const WITHDRAW_RULES = { fee: 400_000_000n, min: 1_190_000n, max: 50_000_000_000_000n, coreFeeDuffs: 190 } as const;

/** What a withdrawal of `credits` pays out (duffs). */
export const withdrawPayout = (credits: bigint) => Number(credits / 1000n) - WITHDRAW_RULES.coreFeeDuffs;

/** A withdrawal document id (Base58, 44 characters). */
export const withdrawalId = (n: number) => `Wd${String(n).padStart(4, "0")}`.padEnd(44, "x");

/** The fee bound a top-up's funding pays (duffs), as the registration quote's. */
export const TOP_UP_FEE_DUFFS = 2_260;

/** A top-up funding step, as the engine names it (`topup/<index>-<k>/funding`). */
export const topUpStep = (index: number, k: number) => `topup/${index}-${k}/funding`;

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

export interface TopUpDeps {
  wallet(w: World, walletId: string): FixtureWallet;
  dashpay(w: World, walletId: string, identity?: string): DashPayWorld;
  grant(w: World, id: string, walletId: string, kinds: ["platformOp"]): FixtureGrant | null;
  useGrant(w: World, id: string): void;
  costs: CostTable;
}

export const creditsOf = (dp: DashPayWorld): CreditsWorld => (dp.credits ??= newCreditsWorld());

/** `contest_status` with no contest set: the status's own contest, its votes counted. */
function derivedContest(dp: DashPayWorld, identity: string, label: string): ContestStatus {
  const s = dp.status;
  if (s.kind !== "contestPending" || s.identity !== identity || s.label !== label) throw fail("invalid_argument", "no contest of that label", { kind: "reason", reason: "no contest" });
  return {
    label,
    state: { kind: "open" },
    endsAt: s.endsAt,
    contenders: [
      { identity, votes: 12, isSelf: true },
      { identity: IDENTITY.mallory, votes: 4, isSelf: false },
    ],
    lockVotes: 2,
    abstainVotes: 1,
    temporaryName: null,
  };
}

/** The engine's plan (withdrawal.rs `Rules::credits`): Max is the balance less the fee in whole
 * duffs, capped at the maximum; an explicit amount must lie in the range and leave the fee. */
function withdrawPlan(balance: bigint, amount: WithdrawAmount): bigint {
  const R = WITHDRAW_RULES;
  if (amount.kind === "all") {
    const free = balance > R.fee ? balance - R.fee : 0n;
    const whole = free - (free % 1000n);
    const credits = whole < R.max ? whole : R.max;
    if (credits < R.min) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(R.min + R.fee), available: String(balance) });
    return credits;
  }
  const credits = BigInt(amount.credits);
  if (credits < R.min || credits > R.max) throw fail("credits.withdrawal_out_of_range", "out of range", { kind: "creditRange", min: String(R.min), max: String(R.max) });
  if (balance < credits + R.fee) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(credits + R.fee), available: String(balance) });
  return credits;
}

export function topUpHandlers(d: TopUpDeps): Handlers {
  const changed = ({ world, emit }: Ctx, walletId: string) =>
    emit({ kind: "platform", network: world.launch.network.dirName, walletId, change: { domain: "identities" } });

  /** The quote's refusals, in the engine's order; the total once nothing refuses. */
  function check(world: World, walletId: string, identity: string, duffs: number) {
    const dp = d.dashpay(world, walletId, identity);
    const me = dp.identities.find((i) => i.identity === identity)!;
    const c = creditsOf(dp);
    if (duffs < d.costs.topUpMinDuffs) throw fail("credits.below_minimum", "below the minimum", { kind: "limit", duffs: d.costs.topUpMinDuffs });
    const pending = c.pending.find((p) => p.identityIndex === me.index);
    if (pending) throw fail("credits.top_up_pending", "a top-up is pending", { kind: "txid", txid: pending.txid });
    const totalDuffs = duffs + TOP_UP_FEE_DUFFS;
    const available = fundable(d.wallet(world, walletId));
    if (available < totalDuffs) throw fail("credits.funding_insufficient", "funding insufficient", { kind: "duffs", needed: totalDuffs, available });
    return { dp, me, c, totalDuffs };
  }

  /** What a top-up of `duffs` adds: its lock in credits less the table's fee. */
  const creditsFor = (duffs: number) => BigInt(duffs) * BigInt(d.costs.creditsPerDuff) - BigInt(d.costs.topUpFee);

  function apply(ctx: Ctx, walletId: string, dp: DashPayWorld, identity: string, added: bigint): string {
    let balance = "0";
    dp.identities = dp.identities.map((i) => {
      if (i.identity !== identity) return i;
      balance = String(BigInt(i.balance ?? "0") + added);
      return { ...i, balance, balanceStale: false, creditLevel: BigInt(balance) < BigInt(d.costs.emptyCredits) ? "empty" : BigInt(balance) <= BigInt(d.costs.lowCredits) ? "low" : "ok" };
    });
    changed(ctx, walletId);
    return balance;
  }

  return {
    // The answer arrives as `Platform{Identities}` in the engine; the fixture's balance is current.
    identity_refresh_balance: ({ walletId, identity }, { world }) => d.dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!.balance,
    contest_status: ({ walletId, identity, label }, { world }) => {
      const dp = d.dashpay(world, walletId, identity);
      const set = creditsOf(dp).contest;
      return set && set.label === label ? set : derivedContest(dp, identity, label);
    },
    credits_top_up_quote: ({ walletId, identity, duffs }, { world }) => {
      const { totalDuffs } = check(world, walletId, identity, duffs);
      return { feeDuffs: TOP_UP_FEE_DUFFS, totalDuffs, credits: String(creditsFor(duffs)), grant: { maxDuffs: totalDuffs, maxCredits: "0" } };
    },
    credits_top_up: ({ walletId, identity, duffs, grant }, ctx) => {
      const { world } = ctx;
      const { dp, me, c, totalDuffs } = check(world, walletId, identity, duffs);
      const stuck = c.stuck.find((s) => s.identityIndex === me.index);
      if (stuck) throw fail(stuck.state === "willBeSent" ? "platform.will_be_sent" : "platform.broadcast_unknown", "funding outcome unknown", { kind: "artifact", artifact: stuck.step });
      const g = d.grant(world, grant, walletId, ["platformOp"]);
      if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
      if (g.grant.purpose.maxDuffs < totalDuffs)
        throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "funding", needed: String(totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
      d.useGrant(world, grant);
      if (c.next === "broadcastUnknown") {
        c.locks += 1;
        const step = topUpStep(me.index, c.locks);
        c.stuck = [...c.stuck, { step, identityIndex: me.index, identity, state: "maybeSent" }];
        throw fail("platform.broadcast_unknown", "funding outcome unknown", { kind: "artifact", artifact: step });
      }
      c.locks += 1;
      const txid = (0xa0 + c.locks).toString(16).repeat(32);
      const added = creditsFor(duffs);
      let state: TopUpState;
      switch (c.next) {
        case "applied":
        case "alreadyApplied":
          state = { kind: "applied", creditsAdded: String(added), balance: apply(ctx, walletId, dp, identity, added) };
          break;
        case "consumptionUnknown":
          state = { kind: "consumptionUnknown", balance: me.balance };
          break;
        case "proofWaiting":
          state = { kind: "proofWaiting" };
          break;
        case "notApplied":
          state = { kind: "notApplied", error: "platform.unavailable" };
          break;
      }
      if (state.kind !== "applied")
        c.pending = [...c.pending, { txid, identity, identityIndex: me.index, duffs, proved: state.kind !== "proofWaiting", consumptionUnknown: state.kind === "consumptionUnknown" }];
      changed(ctx, walletId);
      return { txid, state } satisfies TopUpOutcome;
    },
    credits_pending_top_ups: ({ walletId }, { world }) => creditsOf(d.dashpay(world, walletId)).pending,
    credits_stuck_top_up_fundings: ({ walletId }, { world }) => creditsOf(d.dashpay(world, walletId)).stuck,
    // Applies the lock: the fixture's proof always arrives in the resume's wait.
    credits_resume_top_up: ({ walletId, txid, grant }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId);
      const c = creditsOf(dp);
      const row = c.pending.find((p) => p.txid === txid);
      if (!row) throw fail("invalid_argument", "no top-up lock has that txid", { kind: "reason", reason: "no top-up lock has that txid" });
      const g = d.grant(ctx.world, grant, walletId, ["platformOp"]);
      if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs < 1) throw fail("platform.grant_invalid", "grant invalid");
      d.useGrant(ctx.world, grant);
      c.pending = c.pending.filter((p) => p !== row);
      const added = creditsFor(row.duffs);
      const identity = row.identity ?? dp.identities.find((i) => i.index === row.identityIndex)!.identity;
      return { txid, state: { kind: "applied", creditsAdded: String(added), balance: apply(ctx, walletId, dp, identity, added) } };
    },
    credits_withdraw_quote: ({ walletId, identity, amount }, { world }) => {
      const me = d.dashpay(world, walletId, identity).identities.find((i) => i.identity === identity)!;
      const credits = withdrawPlan(BigInt(me.balance ?? "0"), amount);
      return { credits: String(credits), feeCredits: String(WITHDRAW_RULES.fee), expectedDuffs: withdrawPayout(credits), grant: { maxDuffs: 0, maxCredits: String(credits + WITHDRAW_RULES.fee) } };
    },
    // The engine's order (m4 §2.8): the destination, the plan on the balance, the grant, then the
    // broadcast. Platform's fee is the reserve here; the real one is metered and lower.
    credits_withdraw: ({ walletId, identity, to, amount, grant }, ctx) => {
      const { world } = ctx;
      const dp = d.dashpay(world, walletId, identity);
      const me = dp.identities.find((i) => i.identity === identity)!;
      if (classifyAddress(world.launch.network.dirName, to).kind !== "core")
        throw fail("invalid_argument", "not a Dash address of the network", { kind: "reason", reason: "not a Dash address of the network" });
      const credits = withdrawPlan(BigInt(me.balance ?? "0"), amount);
      const total = credits + WITHDRAW_RULES.fee;
      const expectedDuffs = withdrawPayout(credits);
      const g = d.grant(world, grant, walletId, ["platformOp"]);
      if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
      if (BigInt(g.grant.purpose.maxCredits) < total)
        throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(total), remaining: g.grant.purpose.maxCredits });
      d.useGrant(world, grant);
      const c = creditsOf(dp);
      const id = withdrawalId(c.withdrawals.length + 1);
      if (c.nextWithdraw === "unknown") throw fail("platform.broadcast_unknown", "withdrawal outcome unknown", { kind: "artifact", artifact: id });
      const at = FIXTURE_NOW.getTime() + c.withdrawals.length * 1000;
      c.withdrawals = [{ id, credits: String(credits), expectedDuffs, to: to.trim(), status: "queued", createdAt: at, updatedAt: at }, ...c.withdrawals];
      const remaining = apply(ctx, walletId, dp, identity, -total);
      return { credits: String(credits), expectedDuffs, remainingCredits: c.nextWithdraw === "landed" ? null : remaining, withdrawal: id } satisfies WithdrawOutcome;
    },
    credits_withdrawals: ({ walletId, identity }, { world }) => creditsOf(d.dashpay(world, walletId, identity)).withdrawals,
    credits_resolve_top_up: ({ walletId, txid }, ctx) => {
      const c = creditsOf(d.dashpay(ctx.world, walletId));
      const row = c.pending.find((p) => p.txid === txid && p.proved);
      if (!row) throw fail("invalid_argument", "no pending top-up of that txid is proved", { kind: "reason", reason: "no pending top-up of that txid is proved" });
      c.pending = c.pending.filter((p) => p !== row);
      changed(ctx, walletId);
      return null;
    },
  };
}
