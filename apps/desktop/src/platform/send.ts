// Pay from Platform (0.2 X1b; contract m4 §2.12 `platform_send_quote`, `platform_send`): Pay's
// address tab, given a Platform address, pays it from the wallet's Platform addresses. The quote
// names the amount, the fee estimate and the address the fee is taken from, and Confirm runs
// `platform_send` for the quote's single-use operation under a grant issued for that operation. One flow
// per wallet, kept while the app runs (as move.ts), so leaving Pay while a send runs keeps its result.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { CREDITS_PER_DUFF } from "../api/pending";
import { failureText, Model } from "../onboarding/model";
import { t } from "../i18n";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { invalidArgumentText, isHeldOp, moveCanRetry, moveErrorText, onMoveSettledNotify, preflightText, stillHeld } from "./move";
import { isAddressSendsOff } from "./mainnetOff";

export type PlatformSendApi = Pick<Backend, "platformSend" | "platformSendQuote" | "authorize" | "authorizePlatformSend" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const SEND_COPY = {
  get title() {
    return t("pay.platform.title");
  },
  get intro() {
    return t("pay.platform.intro");
  },
  from: (balance: string) => t("pay.platform.from", { balance }),
  get empty() {
    return t("pay.platform.empty");
  },
  get max() {
    return t("pay.platform.max");
  },
  /** The form's fee line: the estimate; Max pays it from the amount. Platform meters the fee, so it is no maximum. */
  fee: (fee: string, fromAmount: boolean) => (fromAmount ? t("pay.platform.feeFromAmount", { fee }) : t("pay.platform.fee", { fee })),
  get quoting() {
    return t("pay.platform.quoting");
  },
  rows: {
    get from() {
      return t("pay.platform.rows.from");
    },
    get fromValue() {
      return t("pay.platform.rows.fromValue");
    },
    get to() {
      return t("pay.platform.rows.to");
    },
    get amount() {
      return t("pay.platform.rows.amount");
    },
    get fee() {
      return t("pay.platform.rows.fee");
    },
    get total() {
      return t("pay.platform.rows.total");
    },
    get feeSource() {
      return t("pay.platform.rows.feeSource");
    },
    get totalExact() {
      return t("pay.platform.rows.totalExact");
    },
    get recipientGets() {
      return t("pay.platform.rows.recipientGets");
    },
    amountEverything: (amount: string) => t("pay.platform.rows.amountEverything", { amount }),
  },
  feeValue: (fee: string) => t("pay.platform.feeValue", { fee }),
  /** Where Platform takes the fee from, and what it can reach there: what that address keeps (`keeps`), and more if
   * anything is sent to it before the payment goes through. */
  feeSource: (fee: string, keeps: string) => t("pay.platform.feeSource", { fee, keeps }),
  get feeFromAmount() {
    return t("pay.platform.feeFromAmountNote");
  },
  get confirmText() {
    return t("pay.platform.confirmText");
  },
  get cancel() {
    return t("common.cancel");
  },
  get go() {
    return t("pay.platform.go");
  },
  running: {
    get title() {
      return t("pay.platform.running.title");
    },
    get text() {
      return t("pay.platform.running.text");
    },
  },
  done: {
    get title() {
      return t("pay.platform.done.title");
    },
    text: (taken: string, about: boolean) => (about ? t("pay.platform.done.textAbout", { taken }) : t("pay.platform.done.text", { taken })),
  },
  pending: {
    get title() {
      return t("pay.platform.pending.title");
    },
    get text() {
      return t("pay.platform.pending.text");
    },
  },
  failed: {
    get title() {
      return t("pay.platform.failed.title");
    },
    get text() {
      return t("pay.platform.failed.text");
    },
  },
  replaced: {
    get title() {
      return t("pay.platform.replaced.title");
    },
    get text() {
      return t("pay.platform.replaced.text");
    },
  },
  /** The payment's group settled with no evidence of which member went through: never "Sent". */
  settledUnknown: {
    get title() {
      return t("pay.status.settledUnknown.title");
    },
    get text() {
      return t("pay.status.settledUnknown.text");
    },
  },
  /** How an unsettled payment ended, by Activity or by the send's own answer. */
  settled: {
    went: {
      get title() {
        return t("pay.platform.settled.went.title");
      },
      get text() {
        return t("pay.platform.settled.went.text");
      },
    },
    /** Platform refused the payment for good (failed, and it can no longer land). */
    refused: {
      get title() {
        return t("pay.platform.settled.refused.title");
      },
      get text() {
        return t("pay.platform.settled.refused.text");
      },
    },
  },
  /** A retry was asked for but the engine did not answer it as that payment's retry. */
  get noRetry() {
    return t("pay.platform.noRetry");
  },
  get waiting() {
    return t("pay.platform.waiting");
  },
  /** `credits.move_pending` from the quote or the send, by what holds: a payment, which Retry that
   * payment re-sends; a move, which only Move to Dash wallet follows; or one Activity does not show. */
  get held() {
    return t("pay.platform.held");
  },
  get heldMove() {
    return t("pay.platform.heldMove");
  },
  get heldOther() {
    return t("pay.platform.heldOther");
  },
  /** `invalid_argument` from the send: its operation is unknown or used. */
  get expired() {
    return t("pay.platform.expired");
  },
  get quoteChanged() {
    return t("pay.platform.quoteChanged");
  },
  get reviewAgain() {
    return t("pay.platform.reviewAgain");
  },
  get retry() {
    return t("pay.platform.retry");
  },
  get retrying() {
    return t("pay.platform.retrying");
  },
  get back() {
    return t("pay.platform.back");
  },
  get close() {
    return t("pay.platform.close");
  },
  get view() {
    return t("pay.platform.view");
  },
  /** A Confirm taken back to the form because Activity moved on. */
  get went() {
    return t("pay.platform.went");
  },
  /** Confirm shows it when the quote is the exact retry of an earlier payment. */
  get retryNote() {
    return t("pay.platform.retryNote");
  },
  /** The form: an earlier payment that may still go through, and its retry. */
  earlier: (amount: string, to: string) => t("pay.platform.earlier", { amount, to }),
  get retryThat() {
    return t("pay.platform.retryThat");
  },
  get gone() {
    return t("pay.platform.gone");
  },
};

/** Why `credits.recipient_invalid` refused the address (its `reason`). */
export function recipientText(reason: string | undefined): string {
  switch (reason) {
    case "core":
      return t("pay.platform.recipient.core");
    case "shielded":
      return t("pay.platform.recipient.shielded");
    case "network":
      return t("pay.platform.recipient.network");
    case "own":
      return t("pay.platform.recipient.own");
    default:
      return t("pay.platform.recipient.invalid");
  }
}

/** A send's refusal, by code; the move's texts for the codes both share. `retryOf`: the held send this
 * one retried; while it still holds, a pre-check refusal says it may still land. */
export function sendErrorText(e: unknown, credits: (c: string) => string, activity?: readonly PlatformActivity[] | null, retryOf?: string | null): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.rules_unsupported":
    case "credits.preflight_refused":
      return preflightText(e.code, stillHeld(activity, retryOf, "send") ? "send" : null);
    case "credits.recipient_invalid":
      return recipientText(e.data?.kind === "reason" ? e.data.reason : undefined);
    case "credits.send_out_of_range":
      // Also when one of the addresses it takes from would give less than that minimum.
      return e.data?.kind === "creditMinimum"
        ? t("pay.platform.outOfRangeMin", { min: credits(e.data.min) })
        : t("pay.platform.outOfRange");
    case "credits.move_pending": {
      const id = e.data?.kind === "artifact" ? e.data.artifact : null;
      switch (activity?.find((a) => a.id === id)?.kind) {
        case "send":
          return SEND_COPY.held;
        case "moveToCore":
          return SEND_COPY.heldMove;
        default:
          return SEND_COPY.heldOther;
      }
    }
    case "credits.quote_changed":
      return SEND_COPY.quoteChanged;
    case "platform.trust_mismatch":
      return t("pay.platform.trustMismatch");
    case "invalid_argument":
      return invalidArgumentText(e);
    default:
      return moveErrorText(e, credits);
  }
}

/** The refusal of `platformSend` itself: an unknown or used operation (`invalid_argument`) means the
 * quote it came from is no good any more; the rest as a quote's. */
export function sendRefusalText(e: unknown, credits: (c: string) => string, activity?: readonly PlatformActivity[] | null, retryOf?: string | null): string {
  return hasCode(e, "invalid_argument") ? SEND_COPY.expired : sendErrorText(e, credits, activity, retryOf);
}

/** Back is offered when a new quote may pass (as for a move), after an expired operation, and when
 * the balances changed since the review (the form has quoted again). */
export const sendCanRetry = (e: unknown) => moveCanRetry(e) || hasCode(e, "invalid_argument") || hasCode(e, "credits.quote_changed");

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
 * `went` when its row is done or replaced, `refused` when it failed for good, `unknown` when its group
 * settled with no evidence of which member went through; `null` while it may still land. */
export type Settled = "went" | "refused" | "unknown";
export function settledBy(outcome: Pick<PlatformSendOutcome, "id" | "mayLand"> | null, activity: readonly PlatformActivity[] | null | undefined): Settled | null {
  if (!outcome || !outcome.mayLand || !activity) return null;
  const row = activity.find((a) => a.id === outcome.id);
  if (!row) return null;
  if (row.winnerKnown === false) return "unknown";
  if (row.state === "done" || row.state === "replaced") return "went";
  return row.state === "failed" && !row.mayLand ? "refused" : null;
}

/** The send went through: done, or settled as went, and never when its group names no winner. */
export const sendWent = (outcome: Pick<PlatformSendOutcome, "state" | "winnerKnown"> | null | undefined, settled: Settled | null): boolean =>
  settled !== "unknown" && outcome?.winnerKnown !== false && (outcome?.state === "done" || settled === "went");

/** What the result sheet says: the outcome's own state, unless Activity has since settled it (`settled`).
 * `retried`: the send answered a retry, which may be the earlier payment's own outcome. */
export function sendOutcomeCopy(outcome: PlatformSendOutcome, credits: (c: string) => string, settled: Settled | null, retried: boolean): { title: string; text: string } {
  if (settled === "unknown" || outcome.winnerKnown === false) return SEND_COPY.settledUnknown;
  if (settled === "went") return SEND_COPY.settled.went;
  if (settled === "refused") return SEND_COPY.settled.refused;
  switch (outcome.state) {
    case "done": {
      // The balances' figure: a deposit to the fee source while it settled hides part of the fee. Max's inputs gave
      // exactly their balances (the output paid the fee), so its debit is exact.
      const text = SEND_COPY.done.text(credits(outcome.credits), !outcome.feeFromAmount);
      return { title: SEND_COPY.done.title, text: retried ? `${SEND_COPY.went} ${text}` : text };
    }
    case "pending":
      return SEND_COPY.pending;
    case "failed":
      return outcome.mayLand ? SEND_COPY.failed : SEND_COPY.settled.refused;
    case "replaced":
      return SEND_COPY.replaced;
  }
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

  /** Confirm & send: a grant issued for the quote's operation, then `platformSend` for it
   * (one call at a time). The operation is single use, so any attempt that ran spends it. */
  async send(quote: PlatformSendQuote): Promise<void> {
    if (this.running) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant({ kind: "platformSend", operation: quote.operation }, this.walletId, (g: AuthGrant) => {
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
          // Activity cannot tell Max from an exact amount: the next ask is Max. On mainnet the flag
          // answers an ask that is not the held send's exact terms.
          if (i < asks.length - 1 && (hasCode(e, "invalid_argument") || isAddressSendsOff(e))) continue;
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
