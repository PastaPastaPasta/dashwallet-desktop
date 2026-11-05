// Pay from Platform (0.2 X1b; contract m4 §2.12 `platform_send_quote`, `platform_send`): Pay's
// address tab, given a Platform address, pays it from the wallet's Platform addresses. The quote
// names the amount, the fee estimate and the most the fee can take, and Confirm runs
// `platform_send` for the quote's single-use operation under a fresh `platformOp` grant. One flow
// per wallet, kept while the app runs (as move.ts), so leaving Pay while a send runs keeps its result.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { CREDITS_PER_DUFF } from "../api/pending";
import { platformOp } from "../dashpay/topup";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { invalidArgumentText, isHeldOp, moveCanRetry, moveErrorText, onMoveSettledNotify } from "./move";

export type PlatformSendApi = Pick<Backend, "platformSend" | "platformSendQuote" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const SEND_COPY = {
  title: "Pay from Platform",
  intro: "This is a Dash Platform address. It is paid from your Platform balance.",
  from: (balance: string) => `From your Platform balance: ${balance}`,
  empty: "Your Platform balance is empty. Paying a Platform address from your Dash wallet is not supported yet.",
  max: "Everything on your Platform addresses. The fee comes out of the amount.",
  /** The form's fee line: the estimate and the most the fee can take; Max pays it from the amount. */
  fee: (fee: string, atMost: string | null) => (atMost === null ? `Platform fee about ${fee}, from the amount` : `Platform fee about ${fee} (at most ${atMost})`),
  quoting: "Working out the fee…",
  rows: {
    from: "From",
    fromValue: "Your Platform balance",
    to: "To",
    amount: "Amount",
    everything: "everything",
    fee: "Platform fee",
    total: "Taken from Platform, at most",
    totalExact: "Taken from Platform",
    recipientGets: "Recipient gets, about",
  },
  /** The estimate, and the most the fee can take (all its source address keeps after the payment). */
  feeValue: (fee: string, atMost: string | null) => (atMost === null ? `about ${fee}` : `about ${fee}, at most ${atMost}`),
  feeSource: "Platform takes its fee from one of your Platform addresses: at most what that address keeps after the payment.",
  feeFromAmount: "The fee comes out of the amount: the recipient gets the amount less the fee, which may be more than the estimate.",
  confirmText: "Dash Platform pays the address below. This cannot be undone.",
  cancel: "Cancel",
  go: "Confirm & send",
  running: { title: "Sending…", text: "Dash Platform confirms the payment. You can close this: Activity shows it." },
  done: {
    title: "Sent",
    text: (taken: string, about: boolean) => `${about ? "About " : ""}${taken} left your Platform balance, the fee included. Activity shows the payment.`,
  },
  pending: {
    title: "Waiting for Platform",
    text: "Dash Platform has not said whether the payment went through. Activity shows it when it does. Do not pay again until it does.",
  },
  failed: {
    title: "Did not go through",
    text: "Dash Platform did not take the payment in time. It may still go through later; Activity shows it if it does. Until it settles, only this same payment can be sent again.",
  },
  replaced: { title: "Replaced by a retry", text: "This payment was replaced by a retry. Activity shows the retry." },
  /** How an unsettled payment ended, by Activity or by the send's own answer. */
  settled: {
    went: { title: "Sent", text: "That payment went through. Activity shows it." },
    /** Platform refused the payment for good (failed, and it can no longer land). */
    refused: { title: "Not sent", text: "Dash Platform refused this payment. Nothing was sent; you can send it again." },
  },
  /** A retry was asked for but the engine did not answer it as that payment's retry. */
  noRetry: "This payment can no longer be retried from here. Check Activity, then send it again if it did not go through.",
  waiting: "A payment or move from Platform is not confirmed yet. Pay again once Activity shows that it went through.",
  /** `credits.move_pending` from the quote or the send: an earlier one may still go through. */
  held: "An earlier payment or move from Platform may still go through. Wait for it to finish, or pay the same address the same amount again: only that retry can go now.",
  /** `invalid_argument` from the send: its operation is unknown or used. */
  expired: "This payment's details expired. Review it again.",
  retry: "Retry this payment",
  retrying: "Working out the fee…",
  back: "Back",
  close: "Close",
  view: "View in Activity",
  /** A Confirm taken back to the form because Activity moved on. */
  went: "That payment went through.",
  /** Confirm shows it when the quote is the exact retry of an earlier payment. */
  retryNote: "This retries your earlier payment. If that one went through, nothing more is sent.",
  /** The form: an earlier payment that may still go through, and its retry. */
  earlier: (amount: string, to: string) => `An earlier payment of ${amount} to ${to} may still go through.`,
  retryThat: "Retry that payment",
  gone: "That payment can no longer go through. Check the amount and send again.",
} as const;

/** Why `credits.recipient_invalid` refused the address (its `reason`). */
export function recipientText(reason: string | undefined): string {
  switch (reason) {
    case "core":
      return "This is a Dash address. Pay it from your Dash wallet.";
    case "shielded":
      return "This is a shielded address. This wallet cannot pay shielded addresses from Platform yet.";
    case "network":
      return "This Platform address is for another network.";
    case "own":
      return "This is one of your own Platform addresses.";
    default:
      return "This is not a valid Platform address. Please recheck.";
  }
}

/** A send's refusal, by code; the move's texts for the codes both share. */
export function sendErrorText(e: unknown, credits: (c: string) => string): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.recipient_invalid":
      return recipientText(e.data?.kind === "reason" ? e.data.reason : undefined);
    case "credits.send_out_of_range":
      // Also when one of the addresses it takes from would give less than that minimum.
      return e.data?.kind === "creditMinimum"
        ? `Dash Platform cannot send this amount: the payment, and what it takes from each of your Platform addresses, must be at least ${credits(e.data.min)}. Try another amount, or Max.`
        : "Dash Platform does not take a payment of this amount.";
    case "credits.move_pending":
      return SEND_COPY.held;
    case "platform.trust_mismatch":
      return "Your Platform addresses could not be verified against the network. Nothing was sent.";
    case "invalid_argument":
      return invalidArgumentText(e);
    default:
      return moveErrorText(e, credits);
  }
}

/** The refusal of `platformSend` itself: an unknown or used operation (`invalid_argument`) means the
 * quote it came from is no good any more; the rest as a quote's. */
export function sendRefusalText(e: unknown, credits: (c: string) => string): string {
  return hasCode(e, "invalid_argument") ? SEND_COPY.expired : sendErrorText(e, credits);
}

/** Back is offered when a new quote may pass (as for a move), and after an expired operation. */
export const sendCanRetry = (e: unknown) => moveCanRetry(e) || hasCode(e, "invalid_argument");

/** The amount a send asks for: Max is `all` (the output pays the fee); otherwise the duffs as credits. */
export function sendAmount(max: boolean, duffs: number | null | undefined): WithdrawAmount | null {
  if (max) return { kind: "all" };
  if (typeof duffs !== "number" || duffs <= 0) return null;
  return { kind: "credits", credits: String(BigInt(duffs) * CREDITS_PER_DUFF) };
}

/** The amount a retry of `outcome` asks for: the same payment, so the engine answers the exact retry. */
export function retryAmount(outcome: Pick<PlatformSendOutcome, "feeFromAmount" | "amountCredits">): WithdrawAmount {
  return outcome.feeFromAmount ? { kind: "all" } : { kind: "credits", credits: outcome.amountCredits };
}

/** Whether a result can be retried: it failed by height and may still execute. (A pending one is
 * refused until Platform answers.) */
export const canRetry = (outcome: Pick<PlatformSendOutcome, "state" | "mayLand"> | null): boolean => outcome !== null && outcome.state === "failed" && outcome.mayLand;

/** Why an open Confirm must go back to the form: `went` and `gone` when the held send its quote
 * retries has settled (done, or replaced or failed for good); `held` when a move or send that may
 * land turned up that was not there when it opened (the engine would refuse this send). `opened` is
 * the Platform activity at that moment; `null` while Activity has not answered. */
export type StaleConfirm = "went" | "gone" | "held";
export function staleConfirm(quote: Pick<PlatformSendQuote, "retryOf">, opened: readonly PlatformActivity[] | null | undefined, now: readonly PlatformActivity[] | null | undefined): StaleConfirm | null {
  if (!now) return null;
  if (quote.retryOf !== null) {
    const row = now.find((a) => a.id === quote.retryOf);
    if (!row || row.mayLand || row.state === "pending") return null;
    // A replaced send's identical retry went through.
    return row.state === "done" || row.state === "replaced" ? "went" : "gone";
  }
  const before = new Set((opened ?? []).filter(isHeldOp).map((a) => a.id));
  return now.some((a) => isHeldOp(a) && !before.has(a.id)) ? "held" : null;
}

/** What Activity says of an unsettled outcome (pending, or failed by height) the result sheet shows:
 * `went` when its row is done or replaced, `refused` when it failed for good; `null` while it may still land. */
export type Settled = "went" | "refused";
export function settledBy(outcome: Pick<PlatformSendOutcome, "id" | "mayLand"> | null, activity: readonly PlatformActivity[] | null | undefined): Settled | null {
  if (!outcome || !outcome.mayLand || !activity) return null;
  const row = activity.find((a) => a.id === outcome.id);
  if (!row) return null;
  if (row.state === "done" || row.state === "replaced") return "went";
  return row.state === "failed" && !row.mayLand ? "refused" : null;
}

/** An earlier send of the wallet that failed by height and may still go through (the one Retry applies to). */
export const heldSend = (activity: readonly PlatformActivity[] | null | undefined): PlatformActivity | null => activity?.find((a) => a.kind === "send" && a.state === "failed" && a.mayLand && a.to !== null) ?? null;

/** What a retry of the held send `row` asks for, in order: its credits (the amount), then Max when
 * the engine says that is not its amount (Activity does not say which a send was). */
export const retryAsks = (row: Pick<PlatformActivity, "credits">): WithdrawAmount[] => [{ kind: "credits", credits: row.credits }, { kind: "all" }];

export type SendStep = "confirm" | "running" | "done" | "refused";

export class PlatformSendFlow extends Model {
  step: SendStep = "confirm";
  outcome: PlatformSendOutcome | null = null;
  error: unknown = null;
  running = false;
  /** Sends that ran (their operation is used): the form quotes again before the next Confirm. */
  attempts = 0;
  /** The send the last attempt's quote retried (`null`: a new payment). */
  retryOf: string | null = null;
  /** A retry was asked for and the engine did not answer it as that payment's retry. */
  noRetry = false;
  readonly grants: GrantModel;
  readonly #spent = new Set<string>();

  constructor(
    private readonly api: PlatformSendApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Whether an attempt used `operation`: a quote naming it cannot be confirmed again. */
  isSpent(operation: string): boolean {
    return this.#spent.has(operation);
  }

  /** Back to a fresh form: drops a seen result or refusal (not a running call). */
  reset() {
    if (this.running) return;
    this.step = "confirm";
    this.outcome = null;
    this.error = null;
    this.noRetry = false;
    this.grants.cancel();
    this.changed();
  }

  /** Confirm & send: a fresh grant for the quote's credits, then `platformSend` for its operation
   * (one call at a time). The operation is single use, so any attempt that ran spends it. */
  async send(quote: PlatformSendQuote): Promise<void> {
    if (this.running) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(quote.grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.retryOf = quote.retryOf;
        this.#spent.add(quote.operation);
        this.step = "running";
        this.changed();
        return this.api.platformSend(this.walletId, quote.operation, g.id);
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
      if (ran) this.attempts++;
      // Balances and Activity follow a send as they follow a move; a cancelled passphrase ran nothing.
      if (ran) onMoveSettledNotify();
      this.changed();
    }
  }

  /** Retry this payment: quotes the payment `held` names with `retryOf` = its id, the same recipient
   * and amount (the first of `asks` the engine takes as its exact retry). Resolves the quote only
   * when the engine answers it as that send's retry; otherwise `null` (a refusal is in `error`, a
   * quote that is not the retry sets `noRetry`). On success the flow is back at Confirm. */
  async quoteRetry(held: { id: string; to: string }, asks: WithdrawAmount[]): Promise<{ quote: PlatformSendQuote; amount: WithdrawAmount } | null> {
    if (this.running) return null;
    this.running = true;
    this.noRetry = false;
    this.error = null;
    this.changed();
    let got: { quote: PlatformSendQuote; amount: WithdrawAmount } | null = null;
    try {
      for (const [i, amount] of asks.entries()) {
        try {
          const quote = await this.api.platformSendQuote(this.walletId, held.to, amount, held.id);
          if (quote.retryOf === held.id) got = { quote, amount };
          else this.noRetry = true;
          break;
        } catch (e) {
          // Activity cannot tell Max from an exact amount: the next ask is Max.
          if (i < asks.length - 1 && hasCode(e, "invalid_argument")) continue;
          throw e;
        }
      }
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
    }
    if (got) this.reset();
    else this.changed();
    return got;
  }

  /** Retry this payment from the result sheet: the outcome's own recipient and amount (Max again for Max). */
  retry(fallbackTo: string): Promise<{ quote: PlatformSendQuote; amount: WithdrawAmount } | null> {
    const o = this.outcome;
    if (this.running || !o || !canRetry(o)) return Promise.resolve(null);
    return this.quoteRetry({ id: o.id, to: o.to ?? fallbackTo }, [retryAmount(o)]);
  }
}

const flows = new Map<string, PlatformSendFlow>();

/** The wallet's send flow, created on first use and kept while the app runs. */
export function sendFor(api: PlatformSendApi, walletId: string): PlatformSendFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new PlatformSendFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetPlatformSends() {
  flows.clear();
}
