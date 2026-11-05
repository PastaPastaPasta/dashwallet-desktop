// Pay from Platform (0.2 X1b; contract m4 §2.12 `platform_send_quote`, `platform_send`): Pay's
// address tab, given a Platform address, pays it from the wallet's Platform addresses. The quote
// names the amount, the fee estimate and the most the fee may come to (the library's band), and
// Confirm runs `platform_send` under the quote's `platformOp` grant. One flow per wallet, kept while
// the app runs (as move.ts), so leaving Pay while a send runs keeps its result.

import type { Backend } from "../api/backend";
import { isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { CREDITS_PER_DUFF } from "../api/pending";
import { platformOp } from "../dashpay/topup";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { invalidArgumentText, moveErrorText, onMoveSettledNotify } from "./move";

export type PlatformSendApi = Pick<Backend, "platformSend" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const SEND_COPY = {
  title: "Pay from Platform",
  intro: "This is a Dash Platform address. It is paid from your Platform balance.",
  from: (balance: string) => `From your Platform balance: ${balance}`,
  empty: "Your Platform balance is empty. Paying a Platform address from your Dash wallet is not supported yet.",
  max: "Everything on your Platform addresses. The fee comes out of the amount.",
  fee: (fee: string, upTo: string) => `Platform fee about ${fee} (up to ${upTo})`,
  quoting: "Working out the fee…",
  rows: { from: "From", fromValue: "Your Platform balance", to: "To", amount: "Amount", fee: "Platform fee", total: "Taken from Platform, about" },
  /** The estimate, and the most Platform is expected to charge (the library's band on it). */
  feeValue: (fee: string, upTo: string) => `about ${fee}, up to ${upTo}`,
  feeFromAmount: "The fee comes out of the amount: the recipient gets the amount less the fee.",
  confirmText: "Dash Platform pays the address below. This cannot be undone.",
  cancel: "Cancel",
  go: "Confirm & send",
  running: { title: "Sending…", text: "Dash Platform confirms the payment. You can close this: Activity shows it." },
  done: { title: "Sent", text: (taken: string) => `${taken} left your Platform balance, the fee included. Activity shows the payment.` },
  pending: {
    title: "Waiting for Platform",
    text: "Dash Platform has not said whether the payment went through. Activity shows it when it does. Do not pay again until it does.",
  },
  failed: {
    title: "Did not go through",
    text: "Dash Platform did not take the payment in time. It may still go through later; Activity shows it if it does. Until it settles, only this same payment (same address and amount) can be sent again.",
  },
  replaced: { title: "Replaced by a retry", text: "This payment was replaced by a retry. Activity shows the retry." },
  waiting: "A payment or move from Platform is not confirmed yet. Pay again once Activity shows that it went through.",
  /** `credits.move_pending` from the send: an earlier one may still go through. */
  held: "An earlier payment or move from Platform may still go through. Until it settles, only that same payment (same address and amount) can be sent again.",
  back: "Back",
  close: "Close",
  view: "View in Activity",
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

/** Back is offered when a new quote may pass (as for a move). */
export { moveCanRetry as sendCanRetry } from "./move";

/** The amount a send asks for: Max is `all` (the output pays the fee); otherwise the duffs as credits. */
export function sendAmount(max: boolean, duffs: number | null | undefined): WithdrawAmount | null {
  if (max) return { kind: "all" };
  if (typeof duffs !== "number" || duffs <= 0) return null;
  return { kind: "credits", credits: String(BigInt(duffs) * CREDITS_PER_DUFF) };
}

export type SendStep = "confirm" | "running" | "done" | "refused";

export class PlatformSendFlow extends Model {
  step: SendStep = "confirm";
  outcome: PlatformSendOutcome | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: PlatformSendApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Back to a fresh form: drops a seen result or refusal (not a running call). */
  reset() {
    if (this.running) return;
    this.step = "confirm";
    this.outcome = null;
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Confirm & send: the quote's grant, then `platformSend` (one call at a time). */
  async send(grant: GrantRequest, to: string, amount: WithdrawAmount): Promise<void> {
    if (this.running) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.platformSend(this.walletId, to, amount, g.id);
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
      // Balances and Activity follow a send as they follow a move; a cancelled passphrase ran nothing.
      if (ran) onMoveSettledNotify();
      this.changed();
    }
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
