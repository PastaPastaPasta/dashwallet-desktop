// "Add from Dash wallet" (0.2 X1b slice 1; contract m4 §2.12 `platform_fund_quote`, `platform_fund`,
// `pending_platform_funds`, `resume_platform_fund`): Dash from the wallet's Core balance goes to its
// own Platform receive address. The amount is typed, the quote names the fees and what arrives, and
// Confirm runs `platform_fund` under the quote's `platformOp` grant (U-05's GrantModel). A lock that
// was not applied is finished with `resume`. One flow per wallet, kept while the app runs (as
// shielded/flow.ts), so the sheet can close while the call runs and reopen to its result.

import type { Backend } from "../api/backend";
import { isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { PendingPlatformFund } from "../api/generated/PendingPlatformFund";
import type { PlatformFundOutcome } from "../api/generated/PlatformFundOutcome";
import { platformOp } from "../dashpay/topup";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { moveErrorText, onMoveSettledNotify } from "./move";

export type PlatformFundApi = Pick<Backend, "platformFund" | "resumePlatformFund" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export type FundStep = "form" | "confirm" | "running" | "done" | "refused";

/** What the confirm step quotes and sends, or the lock to finish. */
export type FundRequest = { kind: "add"; duffs: number } | { kind: "resume"; txid: string };

export const FUND_COPY = {
  card: {
    unfinished: (amount: string) => `${amount} was locked to add to Platform and is not applied yet.`,
    stuck: {
      tooSmall: (amount: string) => `${amount} was locked to add to Platform, but it's worth less than the Platform fee, so it can't be added.`,
      unsupported: (amount: string) => `${amount} was locked to add to Platform, and this version can't finish it.`,
    },
    finish: "Finish",
    checkAgain: "Check again",
    busy: "Another add to Platform is running.",
  },
  form: {
    intro: "Move Dash from your wallet to your Platform addresses. It is locked first, then Dash Platform adds it to your Platform balance.",
    amount: "Amount",
    invalidAmount: "Enter an amount.",
    why: { amount: "Enter an amount first." },
    next: "Continue",
    cancel: "Cancel",
  },
  confirm: {
    quoting: "Working out the fee…",
    quoteFailed: "The fee could not be worked out.",
    rows: {
      from: "From",
      fromValue: "Your Dash wallet",
      to: "To",
      toValue: "Your Platform address",
      amount: "Amount",
      fee: "Network fee",
      total: "Total from your wallet",
      platformFee: "Platform fee",
      receive: "You receive about",
    },
    back: "Back",
    go: "Confirm & move",
  },
  running: {
    title: "Working…",
    steps: ["Locking funds", "Waiting for the lock to confirm", "Adding to Platform"],
    text: "You can close this: Activity shows it.",
  },
  done: {
    applied: { title: "Added to Platform", text: (amount: string) => `About ${amount} was added to your Platform balance.` },
    waiting: { title: "Locked, not applied yet", text: "Your Dash is locked and waiting for the network's confirmation. Finish it from the Platform card: nothing is lost." },
    checking: { title: "Checking", text: "This amount may already be on your Platform addresses; we can't confirm it yet." },
    notApplied: { title: "Locked, not applied", text: (why: string) => `Your Dash is locked but Dash Platform did not add it yet (${why}). Finish it from the Platform card: nothing is lost.` },
  },
  approvalEnded: {
    pending: (amount: string) =>
      `Your ${amount} is locked, but adding it to Platform stopped before it was applied. Choose Finish: it adds the same locked Dash, not more.`,
    checking: "Checking that add stopped before Platform answered. Choose Check again: it checks the same lock and adds no more Dash.",
    unknown: "Adding to Platform stopped before it finished. If the Platform card shows an add waiting, choose Finish there: it adds the same locked Dash, not more. Otherwise, try again.",
    finish: "Finish",
    checkAgain: "Check again",
  },
  grantInvalid: {
    add: "The approval was not accepted, so nothing was locked. Try again.",
    resume: "The approval was not accepted. Your locked Dash is still waiting: choose Finish again.",
  },
  unseen: "Your add to Platform has an update.",
  show: "Show",
  close: "Close",
  back: "Back",
} as const;

export interface FundRowView {
  kind: "stuck" | "checking" | "unfinished";
  /** The text after the title; `checking` also shows its own title. */
  text: string;
  /** The button, `null` for a lock this version can't finish. */
  action: "finish" | "checkAgain" | null;
}

/** The Platform card's row for an add that is not applied: Finish for one that can be finished, Check
 * again for one that may already be applied, and only the reason for one that can't be finished. */
export function fundRowView(p: Pick<PendingPlatformFund, "duffs" | "state" | "consumptionUnknown">, amount: (duffs: number) => string): FundRowView {
  if (p.state === "tooSmall" || p.state === "unsupported") return { kind: "stuck", text: FUND_COPY.card.stuck[p.state](amount(p.duffs)), action: null };
  if (p.consumptionUnknown) return { kind: "checking", text: FUND_COPY.done.checking.text, action: "checkAgain" };
  return { kind: "unfinished", text: FUND_COPY.card.unfinished(amount(p.duffs)), action: "finish" };
}

/** The approval ended, or the vault locked, between a lock and its apply: the lock waits, unapplied. */
const APPROVAL_ENDED = ["platform.needs_grant", "platform.lease_revoked", "platform.lease_expired"];
const ENDED_BEFORE_APPLY = [...APPROVAL_ENDED, "platform.cancelled"];

/** The request a refusal or outcome answers, and the wallet's adds not applied, when known. */
export interface FundContext {
  request: FundRequest | null;
  pending?: readonly PendingPlatformFund[] | null;
}

/** What a refused Finish or Check again offers to run again: its own lock only, with that lock's row
 * action, never another pending add. A new add's refusal offers nothing (a lock it made comes back as
 * a `notApplied` outcome instead). */
export function finishAfter(e: unknown, { request, pending }: FundContext): { txid: string; duffs: number; action: "finish" | "checkAgain" } | null {
  if (!isAppError(e) || request?.kind !== "resume" || ![...APPROVAL_ENDED, "platform.grant_invalid"].includes(e.code)) return null;
  const own = pending?.find((p) => p.txid === request.txid);
  const action = own ? fundRowView(own, String).action : null;
  return own && action ? { txid: own.txid, duffs: own.duffs, action } : null;
}

/** A "locked, not applied" outcome whose approval ended before the apply: the text to show, and Finish
 * on its own lock; `null` for any other reason. `amount` is the lock's, when known. */
export function endedBeforeApply(error: string, amount: string | null): string | null {
  return ENDED_BEFORE_APPLY.includes(error) ? FUND_COPY.approvalEnded.pending(amount ?? "Dash") : null;
}

/** A refusal of the quote or the call, by code. `credits` shows a credit figure as Dash, `duffs` a duff
 * figure; `ctx` is the request it answers. */
export function fundErrorText(e: unknown, credits: (c: string) => string, duffs: (d: number) => string, ctx: FundContext = { request: null }): string {
  if (!isAppError(e)) return failureText(e);
  const d = e.data;
  if (APPROVAL_ENDED.includes(e.code)) {
    const retry = finishAfter(e, ctx);
    if (!retry) return FUND_COPY.approvalEnded.unknown;
    return retry.action === "finish" ? FUND_COPY.approvalEnded.pending(duffs(retry.duffs)) : FUND_COPY.approvalEnded.checking;
  }
  switch (e.code) {
    case "platform.grant_invalid":
      return ctx.request?.kind === "resume" ? FUND_COPY.grantInvalid.resume : FUND_COPY.grantInvalid.add;
    case "credits.below_minimum":
      return d?.kind === "limit" ? `Too small to add: the smallest amount is ${duffs(d.duffs)}.` : "Too small to add.";
    case "credits.funding_insufficient":
      return d?.kind === "duffs" && d.needed !== null
        ? `Not enough Dash in your wallet: ${duffs(d.needed)} needed with the fee, ${duffs(d.available)} available.`
        : "Not enough Dash in your wallet.";
    case "credits.coins_locked":
      return "Your Dash is reserved right now. Pause mixing or wait for pending payments to finish, then try again.";
    case "credits.too_many_inputs":
      return "That takes too many separate coins at once. Consolidate your coins first, then try again.";
    case "credits.fund_pending":
      return "An earlier add to Platform is not finished. Finish it from the Platform card first.";
    case "platform.feature_off":
      return "Platform addresses are not active on this network yet.";
    case "platform.broadcast_unknown":
      return "The lock may have been sent. Check Activity before trying again.";
    case "platform.will_be_sent":
      return "The payment is still being sent. Try again in a minute.";
    case "platform.signer_unavailable":
      return "Unlock the wallet fully and try again.";
    case "platform.context_unavailable":
      return "Waiting for the network to sync before Platform can be used. Try again shortly.";
    default:
      return moveErrorText(e, credits);
  }
}

/** Back is offered when another amount may pass. */
export const fundCanChange = (e: unknown): boolean =>
  isAppError(e) && ["credits.below_minimum", "credits.funding_insufficient", "platform.grant_exceeded", "credits.too_many_inputs"].includes(e.code);

export class PlatformFundFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: FundStep = "form";
  /** The amount as typed (in the display unit). */
  text = "";
  request: FundRequest | null = null;
  outcome: PlatformFundOutcome | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: PlatformFundApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** A result that came while the sheet was hidden, not yet seen. */
  get unseen(): boolean {
    return !this.open && !this.running && (this.step === "done" || this.step === "refused");
  }

  /** Opens the sheet: at a fresh form, or where a running or unseen call is. */
  begin() {
    this.open = true;
    if (!this.running && this.step !== "done" && this.step !== "refused") {
      this.step = "form";
      this.text = "";
      this.request = null;
      this.outcome = null;
      this.error = null;
    }
    this.changed();
  }

  setText(text: string) {
    this.text = text;
    this.changed();
  }

  /** Form → confirm for an amount the screen has checked. */
  next(duffs: number) {
    this.request = { kind: "add", duffs };
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

  /** Finishes the lock `txid` that was not applied: a grant of one duff (its one-time key), no confirm. */
  async resume(txid: string): Promise<void> {
    this.open = true;
    if (this.running) return this.changed();
    this.step = "running";
    this.outcome = null;
    this.error = null;
    this.request = { kind: "resume", txid };
    this.changed();
    await this.run({ maxDuffs: 1, maxCredits: "0" });
  }

  /** Confirm: the quote's grant, then the call (one at a time). */
  async run(grant: GrantRequest): Promise<void> {
    const request = this.request;
    if (this.running || request === null) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return request.kind === "add" ? this.api.platformFund(this.walletId, request.duffs, g.id) : this.api.resumePlatformFund(this.walletId, request.txid, g.id);
      });
      if (got) {
        this.outcome = got.value;
        this.step = "done";
      } else if (request.kind === "resume") {
        // A cancelled passphrase: there is no form to go back to.
        this.open = false;
        this.step = "form";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
      // A cancelled passphrase ran nothing: nothing to read again.
      if (ran) onMoveSettledNotify();
      this.changed();
    }
  }
}

const flows = new Map<string, PlatformFundFlow>();

/** The wallet's add flow, created on first use and kept while the app runs. */
export function fundFor(api: PlatformFundApi, walletId: string): PlatformFundFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new PlatformFundFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetPlatformFunds() {
  flows.clear();
}
