// The top-up flow (DASHPAY §4 `CreditsViewModel`'s top-up half, F8; DP1-10 T1–T8): the amount,
// the quote's confirm step, the call and its outcome, plus the pending top-ups' Finish and
// "Credits arrived" (D15). One per wallet, kept while the app runs, so the sheet can close while
// the call runs and reopen to its result (D14). Every grant goes through U-05's GrantModel.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CostTable } from "../api/generated/CostTable";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { TopUpOutcome } from "../api/generated/TopUpOutcome";
import { failureText, Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { PROFILE_COPY } from "./profile";
import { COPY } from "./status";

export type TopUpApi = Pick<
  Backend,
  "creditsTopUp" | "creditsResumeTopUp" | "creditsResolveTopUp" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

/** D5: the presets (duffs) and the custom amount's floor. */
export const PRESETS = [5_000_000, 10_000_000] as const;
const MIN_TOP_UP = 1_000_000;

/** The custom amount's floor: 0.01 Dash, or the engine's if higher (D5). */
export const minTopUp = (costs: Pick<CostTable, "topUpMinDuffs"> | undefined) => Math.max(MIN_TOP_UP, costs?.topUpMinDuffs ?? 0);

/** What a top-up of `duffs` adds, as the quote estimates it: the lock in credits less the table's
 * fee, never below 0 (m4 §2.8). */
export const topUpCredits = (duffs: number, costs: Pick<CostTable, "creditsPerDuff" | "topUpFee">): bigint => {
  const credits = BigInt(duffs) * BigInt(costs.creditsPerDuff) - BigInt(costs.topUpFee);
  return credits > 0n ? credits : 0n;
};

/** D15: Finish's grant: the apply signs with the lock's funding key, which a funding budget
 * unlocks; nothing is spent (m4 §2.8). */
export const FINISH_GRANT: GrantRequest = { maxDuffs: 1, maxCredits: "0" };

export const TOPUP_COPY = {
  title: "Top up credits",
  amount: {
    intro: "Credits pay for contact requests and profile changes on Dash Platform.",
    other: "Other amount",
    field: "Amount",
    below: (min: string) => `The smallest top-up is ${min}.`,
    invalid: "Enter an amount.",
    next: "Continue",
    cancel: "Cancel",
  },
  confirm: {
    message: "You have chosen to top up your identity. The following amount will be deducted from your balance.",
    amount: "Amount",
    fee: "Top-up fee",
    total: "Total",
    adds: (credits: string, estimate: string) => `Adds about ${credits} credits: ${estimate}`,
    quoting: "Working out the fee…",
    back: "Back",
    pay: "Confirm & pay",
  },
  running: {
    title: "Adding credits…",
    text: "The network confirms the payment first. This can take a minute. You can close this: My Profile shows the top-up.",
  },
  done: "Done",
  close: "Close",
  repair: COPY.repair.action,
} as const;

export type TopUpStep = "amount" | "confirm" | "running" | "done" | "refused";

/** Which amount T1 has chosen. */
export type AmountChoice = { kind: "preset"; duffs: number } | { kind: "other"; text: string };

/** The outcome screens (T4–T7), and the result line of a pending row's Finish. */
export function outcomeText(o: TopUpOutcome, balance: (credits: string) => string): { title: string; text: string } {
  switch (o.state.kind) {
    case "applied":
    case "alreadyApplied": {
      const added = o.state.kind === "applied" && o.state.creditsAdded !== null ? `+${BigInt(o.state.creditsAdded).toLocaleString("en-US")} credits. ` : "";
      const now = o.state.balance === null ? "" : `Your balance is ${balance(o.state.balance)}.`;
      return { title: "Credits added", text: `${added}${now}`.trim() };
    }
    case "proofWaiting":
      return {
        title: "Payment sent",
        text: "The network has not confirmed the payment yet. Your credits are added once it does; My Profile shows the top-up until then.",
      };
    case "notApplied":
      return { title: "The credits were not added yet", text: "Your payment is safe. Finish the top-up from My Profile." };
    case "consumptionUnknown":
      return {
        title: "Check your credit balance",
        text: "The network says this payment was already used. If your credits went up, choose 'Credits arrived' in My Profile; otherwise Finish tries again.",
      };
  }
}

/** D17: a top-up's refusal, by code. */
export function topUpErrorText(e: unknown, amount: (duffs: number) => string, discreet = false): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.funding_insufficient": {
      if (e.data?.kind !== "duffs" || e.data.needed === null) return "Not enough Dash.";
      const needed = `Not enough Dash: ${amount(e.data.needed)} needed`;
      return discreet ? `${needed}.` : `${needed}, ${amount(e.data.available)} available.`;
    }
    case "credits.below_minimum":
      return e.data?.kind === "limit" ? `The smallest top-up is ${amount(e.data.duffs)}.` : "The amount is below the smallest top-up.";
    case "credits.top_up_pending":
      return PROFILE_COPY.credits.pendingFirst;
    case "credits.coins_locked":
      return "The coins to pay with are held by another payment.";
    case "platform.broadcast_unknown":
      return `${COPY.repair.unknown}. Open Tools ▸ Repair to settle it; you can top up again once it has.`;
    case "platform.will_be_sent":
      return "The payment is still being sent. Try again in a minute.";
    case "platform.context_unavailable":
      return "Waiting for the network to sync and verify this identity. Try again shortly.";
    case "platform.trust_mismatch":
      return "This identity's data could not be verified against the network. Nothing was paid.";
    case "platform.unavailable":
    case "platform.timeout":
      return "Dash Platform did not answer. Try again.";
    case "platform.signer_unavailable":
      return "Unlock the wallet fully and try again.";
    case "platform.grant_exceeded":
      return "The cost went above what you approved.";
    default:
      return `It stopped with ${e.code}.`;
  }
}

/** T8: Back is offered when another amount may pass (D17). */
export const amountCanChange = (e: unknown) =>
  hasCode(e, "credits.funding_insufficient") || hasCode(e, "credits.below_minimum") || hasCode(e, "platform.grant_exceeded");

/** T8: the refusal routes to Tools ▸ Repair (D16). */
export const routesToRepair = (e: unknown) => hasCode(e, "platform.broadcast_unknown");

export const platformOp = (g: GrantRequest) => ({ kind: "platformOp" as const, maxDuffs: g.maxDuffs, maxCredits: g.maxCredits });

/** What a pending row's call came to: its result or refusal, shown under the row. */
export type RowResult = { txid: string; outcome: TopUpOutcome | null; error: unknown };

const settledListeners = new Set<() => void>();

/** D22: called whenever a top-up or a row's call ends, whatever screen is shown (events.ts
 * re-reads `["dashpay"]` from it). Returns the unsubscribe. */
export function onTopUpSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

export class TopUpFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: TopUpStep = "amount";
  /** The identity the flow tops up. */
  identity: string | null = null;
  choice: AmountChoice = { kind: "preset", duffs: PRESETS[0] };
  /** The amount the confirm step quotes. */
  duffs: number | null = null;
  outcome: TopUpOutcome | null = null;
  error: unknown = null;
  /** `top_up` runs (D14); the grant is asked for under it too. */
  running = false;
  /** A pending row's call runs: its txid. */
  rowRunning: string | null = null;
  rowResult: RowResult | null = null;
  /** Locks a call this session answered `consumptionUnknown`: "Credits arrived" is offered for
   * them before the library marks them final (D15, m4 §2.8 `resolve_top_up`). */
  readonly answeredUnknown = new Set<string>();
  readonly grants: GrantModel;

  constructor(
    private readonly api: TopUpApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** A result for `identity` that came while the sheet was hidden, not yet seen (D14): its page offers it.
   *  The flow is per wallet, so another main identity's page does not (D31): its Show would discard it. */
  unseenFor(identity: string): boolean {
    return !this.open && !this.running && this.identity === identity && (this.step === "done" || this.step === "refused");
  }

  /** Opens the sheet for `identity`: at T1, or where a running or unseen call is (D14). */
  begin(identity: string) {
    this.open = true;
    if (this.running || (this.identity === identity && (this.step === "done" || this.step === "refused"))) {
      this.changed();
      return;
    }
    this.identity = identity;
    this.step = "amount";
    this.choice = { kind: "preset", duffs: PRESETS[0] };
    this.duffs = null;
    this.outcome = null;
    this.error = null;
    this.changed();
  }

  choose(choice: AmountChoice) {
    this.choice = choice;
    this.changed();
  }

  /** T1 → T2 for an amount the screen has checked against the floor. */
  next(duffs: number) {
    this.duffs = duffs;
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    if (this.running) return;
    this.step = "amount";
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Hides the sheet; a running call goes on and its result waits (D14). A result seen is
   * dropped. */
  close() {
    this.open = false;
    if (!this.running) {
      this.#dropResult();
      this.grants.cancel();
    }
    this.changed();
  }

  /** Drops an unseen result that a later call or Repair has overtaken (D14). */
  forget() {
    if (this.open || this.running) return;
    this.#dropResult();
    this.changed();
  }

  #dropResult() {
    if (this.step === "done" || this.step === "refused") this.step = "amount";
  }

  #settled(outcome: TopUpOutcome | null) {
    if (outcome?.state.kind === "consumptionUnknown") this.answeredUnknown.add(outcome.txid);
    for (const f of settledListeners) queueMicrotask(f);
  }

  /** Confirm & pay: the quote's grant, then `top_up` (one call at a time, D14). */
  async pay(grant: GrantRequest): Promise<void> {
    const { identity, duffs } = this;
    if (this.running || this.rowRunning !== null || identity === null || duffs === null) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.creditsTopUp(this.walletId, identity, duffs, g.id);
      });
      if (got) {
        this.outcome = got.value;
        this.step = "done";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
      // A cancelled passphrase ran nothing: nothing to read again.
      if (ran) this.#settled(this.step === "done" ? this.outcome : null);
      this.changed();
    }
  }

  /** Finish (D15): `resume_top_up` under a one-duff funding budget. */
  async finish(row: PendingTopUp): Promise<void> {
    await this.#row(row.txid, async () => {
      const got = await this.grants.withGrant(platformOp(FINISH_GRANT), this.walletId, (g) => this.api.creditsResumeTopUp(this.walletId, row.txid, g.id));
      return got ? got.value : null;
    });
  }

  /** "Credits arrived" (D15): `resolve_top_up`, no grant; a proved row only. */
  async creditsArrived(row: PendingTopUp): Promise<void> {
    if (!row.proved) return;
    await this.#row(row.txid, async () => {
      await this.api.creditsResolveTopUp(this.walletId, row.txid);
      this.answeredUnknown.delete(row.txid);
      return null;
    });
  }

  dismissRow() {
    this.rowResult = null;
    this.changed();
  }

  async #row(txid: string, run: () => Promise<TopUpOutcome | null>) {
    if (this.rowRunning !== null || this.running) return;
    this.rowRunning = txid;
    this.rowResult = null;
    // A row's call overtakes the sheet's unseen result (D14).
    if (!this.open) this.#dropResult();
    this.changed();
    try {
      const outcome = await run();
      this.rowResult = outcome ? { txid, outcome, error: null } : null;
    } catch (e) {
      this.rowResult = { txid, outcome: null, error: e };
    } finally {
      this.rowRunning = null;
      this.#settled(this.rowResult?.outcome ?? null);
      this.changed();
    }
  }
}

const flows = new Map<string, TopUpFlow>();

/** The wallet's top-up flow, created on first use and kept while the app runs (D14). */
export function topUpFor(api: TopUpApi, walletId: string): TopUpFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new TopUpFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Repair resolved one of the wallet's top-up fundings: an unseen result is overtaken (D14). */
export function forgetTopUp(walletId: string) {
  flows.get(walletId)?.forget();
}

/** Tests: forget every flow (a fresh app). */
export function resetTopUps() {
  flows.clear();
}
