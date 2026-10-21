// The withdraw flow (DASHPAY §4 `CreditsViewModel`'s withdraw half; DP6-02 W1–W6): the amount
// (Max keeps the network's fee in reserve, the engine's quote), the destination (the wallet's
// receive address unless the user pastes another), the quote's confirm step, the call and its
// outcome, and My Profile's recent withdrawals. One per wallet, kept while the app runs, so the
// sheet can close while the call runs and reopen to its result (as DP1-10 D14). Every grant goes
// through U-05's GrantModel.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CostTable } from "../api/generated/CostTable";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import type { Withdrawal } from "../api/generated/Withdrawal";
import type { WithdrawalStatusName } from "../api/generated/WithdrawalStatusName";
import type { WithdrawOutcome } from "../api/generated/WithdrawOutcome";
import { failureText, Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { creditsToDuffs } from "./profile";

export type WithdrawApi = Pick<Backend, "creditsWithdraw" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const WITHDRAW_COPY = {
  title: "Withdraw credits",
  action: "Withdraw",
  form: {
    intro: "Turn credits back into Dash. Dash Platform sends it to a Dash address.",
    amount: "Amount",
    max: "Max",
    maxLine: "Everything but the network's fee.",
    to: "To",
    toHint: "Your wallet's receive address. You can paste another Dash address.",
    invalidAmount: "Enter an amount.",
    noCosts: "Waiting for the network's prices. Try Max, or wait a moment.",
    invalidTo: "Enter a Dash address.",
    platformTo: "This is a Dash Platform address. Withdraw to a Dash address.",
    next: "Continue",
    cancel: "Cancel",
  },
  confirm: {
    message: "The following credits leave your identity. Dash Platform pays the Dash to the address below.",
    withdraw: "Withdraw",
    fee: "Network fee",
    total: "Taken from credits",
    receive: "You receive about",
    to: "To",
    quoting: "Working out the fee…",
    back: "Back",
    go: "Confirm & withdraw",
  },
  running: {
    title: "Withdrawing…",
    text: "Dash Platform confirms the withdrawal. You can close this: My Profile shows it.",
  },
  done: {
    title: "Withdrawal queued",
    text: (amount: string, to: string) => `${amount} is on its way to ${to}. Dash Platform sends it to the Dash chain, which can take a few minutes.`,
    balance: (balance: string) => `Your credit balance is ${balance}.`,
    landed: "Your credit balance updates shortly.",
  },
  recent: {
    title: "Withdrawals",
    row: (amount: string) => `Withdrawal of ${amount}`,
    status: {
      pending: "Pending",
      complete: "Sent",
      failed: "Failed: the credits were not returned",
      unknown: "Status unknown",
    },
  },
  /** A withdrawal that ended while the sheet was hidden. */
  unseen: "Your withdrawal has an update.",
  show: "Show",
  ok: "Done",
  close: "Close",
} as const;

export type WithdrawStep = "form" | "confirm" | "running" | "done" | "refused";

/** D11: My Profile lists this many; the engine reads 20. */
export const RECENT_SHOWN = 5;

/** D11: how often the recent withdrawals are read while one is pending. */
export const PENDING_POLL_MS = 60_000;

/** The amount the form sends: Max is `all`, so the engine keeps the fee in reserve (D1). An
 * explicit amount is Dash, in credits at the cost table's rate (D10). */
export function withdrawAmount(max: boolean, duffs: number | null, costs: Pick<CostTable, "creditsPerDuff"> | undefined): WithdrawAmount | null {
  if (max) return { kind: "all" };
  if (duffs === null || duffs <= 0 || !costs) return null;
  return { kind: "credits", credits: String(BigInt(duffs) * BigInt(costs.creditsPerDuff)) };
}

/** The query key part of an amount. */
export const amountKey = (a: WithdrawAmount) => (a.kind === "all" ? "all" : a.credits);

/** The status a row shows (§3): Platform keeps re-signing an expired payout, so it is pending. */
export function statusText(s: WithdrawalStatusName): { text: string; pending: boolean } {
  const S = WITHDRAW_COPY.recent.status;
  switch (s) {
    case "queued":
    case "pooled":
    case "broadcasted":
    case "expired":
      return { text: S.pending, pending: true };
    case "complete":
      return { text: S.complete, pending: false };
    case "failed":
      return { text: S.failed, pending: false };
    case "unknown":
      return { text: S.unknown, pending: false };
  }
}

export interface WithdrawalRow {
  id: string;
  title: string;
  status: string;
  pending: boolean;
  to: string | null;
  /** Unix seconds, for the date. */
  at: number | null;
}

/** My Profile's rows: the newest `RECENT_SHOWN` (the engine reads them newest first). */
export function withdrawalRows(list: Withdrawal[] | undefined, costs: Pick<CostTable, "creditsPerDuff"> | undefined, amount: (duffs: number) => string): WithdrawalRow[] {
  return (list ?? []).slice(0, RECENT_SHOWN).map((w) => {
    const s = statusText(w.status);
    const shown = w.expectedDuffs ?? (costs ? creditsToDuffs(w.credits, costs) : null);
    const at = w.updatedAt ?? w.createdAt;
    return {
      id: w.id,
      title: WITHDRAW_COPY.recent.row(shown === null ? `${BigInt(w.credits).toLocaleString("en-US")} credits` : amount(shown)),
      status: s.text,
      pending: s.pending,
      to: w.to,
      at: at === null ? null : Math.floor(at / 1000),
    };
  });
}

/** D11: read again every minute while a row is pending, else only on signals. */
export const pollWhilePending = (list: Withdrawal[] | undefined) => ((list ?? []).slice(0, RECENT_SHOWN).some((w) => statusText(w.status).pending) ? PENDING_POLL_MS : false);

/** A withdrawal's refusal, by code (§3's table). `credits` shows a credit figure as Dash. */
export function withdrawErrorText(e: unknown, credits: (c: string) => string): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? `Withdraw between ${credits(e.data.min)} and ${credits(e.data.max)}.` : "The amount is outside what Dash Platform allows.";
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? `Not enough credits: ${credits(e.data.needed)} needed with the fee, ${credits(e.data.available)} available.` : "Not enough credits.";
    case "identity.keys_missing":
      return "This identity has no transfer key in this wallet, so it cannot withdraw.";
    case "invalid_argument":
      return "Enter a Dash address of this network.";
    case "platform.broadcast_unknown":
      return "It may have been sent. Check your withdrawals in My Profile before trying again.";
    case "platform.context_unavailable":
      return "Waiting for the network to sync and verify this identity. Try again shortly.";
    case "platform.trust_mismatch":
      return "This identity's data could not be verified against the network. Nothing was withdrawn.";
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

/** Back is offered when another amount or address may pass. */
export const amountCanChange = (e: unknown) =>
  hasCode(e, "credits.withdrawal_out_of_range") || hasCode(e, "platform.insufficient_credits") || hasCode(e, "platform.grant_exceeded") || hasCode(e, "invalid_argument");

const platformOp = (g: GrantRequest) => ({ kind: "platformOp" as const, maxDuffs: g.maxDuffs, maxCredits: g.maxCredits });

const settledListeners = new Set<() => void>();

/** Called whenever a withdrawal ends (events.ts re-reads `["dashpay"]` from it): the balance and
 * the recent withdrawals moved. Returns the unsubscribe. */
export function onWithdrawSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

export class WithdrawFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: WithdrawStep = "form";
  /** The identity the flow withdraws from. */
  identity: string | null = null;
  max = true;
  /** The explicit amount as typed, in the display unit. */
  text = "";
  /** The destination as typed; `null`: the wallet's receive address. */
  to: string | null = null;
  /** What the confirm step quotes and sends. */
  amount: WithdrawAmount | null = null;
  destination: string | null = null;
  outcome: WithdrawOutcome | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: WithdrawApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** A result for `identity` that came while the sheet was hidden, not yet seen. */
  unseenFor(identity: string): boolean {
    return !this.open && !this.running && this.identity === identity && (this.step === "done" || this.step === "refused");
  }

  /** Opens the sheet for `identity`: at the form, or where a running or unseen call is. */
  begin(identity: string) {
    this.open = true;
    if (this.running || (this.identity === identity && (this.step === "done" || this.step === "refused"))) {
      this.changed();
      return;
    }
    this.identity = identity;
    this.step = "form";
    this.max = true;
    this.text = "";
    this.to = null;
    this.amount = null;
    this.destination = null;
    this.outcome = null;
    this.error = null;
    this.changed();
  }

  setMax(max: boolean) {
    this.max = max;
    this.changed();
  }

  setText(text: string) {
    this.text = text;
    this.max = false;
    this.changed();
  }

  setTo(to: string) {
    this.to = to;
    this.changed();
  }

  /** Form → confirm for an amount and address the screen has checked. */
  next(amount: WithdrawAmount, destination: string) {
    this.amount = amount;
    this.destination = destination;
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    if (this.running) return;
    this.step = "form";
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Hides the sheet; a running call goes on and its result waits. A result seen is dropped. */
  close() {
    this.open = false;
    if (!this.running) {
      if (this.step === "done" || this.step === "refused") this.step = "form";
      this.grants.cancel();
    }
    this.changed();
  }

  /** Confirm & withdraw: the quote's grant, then `withdraw` (one call at a time). */
  async withdraw(grant: GrantRequest): Promise<void> {
    const { identity, amount, destination } = this;
    if (this.running || identity === null || amount === null || destination === null) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.creditsWithdraw(this.walletId, identity, destination, amount, g.id);
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
      if (ran) for (const f of settledListeners) queueMicrotask(f);
      this.changed();
    }
  }
}

const flows = new Map<string, WithdrawFlow>();

/** The wallet's withdraw flow, created on first use and kept while the app runs. */
export function withdrawFor(api: WithdrawApi, walletId: string): WithdrawFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new WithdrawFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetWithdrawals() {
  flows.clear();
}
