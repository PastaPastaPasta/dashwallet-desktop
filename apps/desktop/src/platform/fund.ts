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
import { t } from "../i18n";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { Preparing } from "../transfer/preparing";
import { isAddressSendsOff, MAINNET_OFF_COPY } from "./mainnetOff";
import { moveErrorText, onMoveSettledNotify } from "./move";

export type PlatformFundApi = Pick<Backend, "platformFund" | "resumePlatformFund" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export type FundStep = "form" | "confirm" | "running" | "done" | "refused";

/** What the confirm step quotes and sends, or the lock to finish. */
export type FundRequest = { kind: "add"; duffs: number } | { kind: "resume"; txid: string };

export const FUND_COPY = {
  card: {
    unfinished: (amount: string) => t("transfer.fund.card.unfinished", { amount }),
    stuck: {
      tooSmall: (amount: string) => t("transfer.fund.card.stuck.tooSmall", { amount }),
      unsupported: (amount: string) => t("transfer.fund.card.stuck.unsupported", { amount }),
    },
    get finish() {
      return t("transfer.fund.card.finish");
    },
    get checkAgain() {
      return t("transfer.fund.card.checkAgain");
    },
    get busy() {
      return t("transfer.fund.card.busy");
    },
  },
  form: {
    get intro() {
      return t("transfer.fund.form.intro");
    },
    get invalidAmount() {
      return t("transfer.fund.form.invalidAmount");
    },
    why: {
      get amount() {
        return t("transfer.fund.form.why.amount");
      },
    },
    get next() {
      return t("transfer.fund.form.next");
    },
    get cancel() {
      return t("common.cancel");
    },
  },
  confirm: {
    get quoting() {
      return t("transfer.fund.confirm.quoting");
    },
    get quoteFailed() {
      return t("transfer.fund.confirm.quoteFailed");
    },
    rows: {
      get from() {
        return t("transfer.fund.confirm.rows.from");
      },
      get fromValue() {
        return t("transfer.fund.confirm.rows.fromValue");
      },
      get to() {
        return t("transfer.fund.confirm.rows.to");
      },
      get toValue() {
        return t("transfer.fund.confirm.rows.toValue");
      },
      get amount() {
        return t("transfer.fund.confirm.rows.amount");
      },
      get fee() {
        return t("transfer.fund.confirm.rows.fee");
      },
      get total() {
        return t("transfer.fund.confirm.rows.total");
      },
      get platformFee() {
        return t("transfer.fund.confirm.rows.platformFee");
      },
      get receive() {
        return t("transfer.fund.confirm.rows.receive");
      },
    },
    get back() {
      return t("transfer.fund.confirm.back");
    },
    get go() {
      return t("transfer.fund.confirm.go");
    },
  },
  running: {
    get title() {
      return t("transfer.fund.running.title");
    },
    get steps() {
      return [t("transfer.fund.running.steps.lock"), t("transfer.fund.running.steps.wait"), t("transfer.fund.running.steps.add")];
    },
    get text() {
      return t("transfer.fund.running.text");
    },
  },
  done: {
    applied: {
      get title() {
        return t("transfer.fund.done.applied.title");
      },
      text: (amount: string) => t("transfer.fund.done.applied.text", { amount }),
    },
    waiting: {
      get title() {
        return t("transfer.fund.done.waiting.title");
      },
      get text() {
        return t("transfer.fund.done.waiting.text");
      },
    },
    checking: {
      get title() {
        return t("transfer.fund.done.checking.title");
      },
      get text() {
        return t("transfer.fund.done.checking.text");
      },
      get row() {
        return t("transfer.fund.done.checking.row");
      },
    },
    notApplied: {
      get title() {
        return t("transfer.fund.done.notApplied.title");
      },
      text: (why: string) => t("transfer.fund.done.notApplied.text", { why }),
    },
  },
  approvalEnded: {
    pending: (amount: string) => t("transfer.fund.approvalEnded.pending", { amount }),
    get checking() {
      return t("transfer.fund.approvalEnded.checking");
    },
    get unknown() {
      return t("transfer.fund.approvalEnded.unknown");
    },
    get finish() {
      return t("transfer.fund.approvalEnded.finish");
    },
    get checkAgain() {
      return t("transfer.fund.approvalEnded.checkAgain");
    },
  },
  grantInvalid: {
    get add() {
      return t("transfer.fund.grantInvalid.add");
    },
    get resume() {
      return t("transfer.fund.grantInvalid.resume");
    },
  },
  get unseen() {
    return t("transfer.fund.unseen");
  },
  get show() {
    return t("transfer.fund.show");
  },
  get close() {
    return t("transfer.fund.close");
  },
  get back() {
    return t("transfer.fund.back");
  },
};

export interface FundRowView {
  kind: "stuck" | "checking" | "unfinished";
  /** The text after the title; `checking` also shows its own title. */
  text: string;
  /** The button, `null` for a lock this version can't finish. */
  action: "finish" | "checkAgain" | null;
}

/** Home's Unfinished transfers row for an add that is not applied: Finish for one that can be finished, Check
 * again for one that may already be applied, and only the reason (Transfer's Add to Platform shows it) for one that
 * can't be finished. */
export function fundRowView(p: Pick<PendingPlatformFund, "duffs" | "state" | "consumptionUnknown">, amount: (duffs: number) => string): FundRowView {
  if (p.state === "tooSmall" || p.state === "unsupported") return { kind: "stuck", text: FUND_COPY.card.stuck[p.state](amount(p.duffs)), action: null };
  if (p.consumptionUnknown) return { kind: "checking", text: FUND_COPY.done.checking.row, action: "checkAgain" };
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
      return d?.kind === "limit" ? t("transfer.fund.error.belowMinimum", { amount: duffs(d.duffs) }) : t("transfer.fund.error.tooSmall");
    case "credits.funding_insufficient":
      return d?.kind === "duffs" && d.needed !== null
        ? t("transfer.fund.error.insufficient", { needed: duffs(d.needed), available: duffs(d.available) })
        : t("transfer.fund.error.insufficientShort");
    case "credits.coins_locked":
      return t("transfer.fund.error.coinsLocked");
    case "credits.too_many_inputs":
      return t("transfer.fund.error.tooManyInputs");
    case "credits.fund_pending":
      return t("transfer.fund.error.pending");
    case "platform.feature_off":
      return isAddressSendsOff(e) ? MAINNET_OFF_COPY.add : t("transfer.fund.error.featureOff");
    case "platform.broadcast_unknown":
      return t("transfer.fund.error.broadcastUnknown");
    case "platform.will_be_sent":
      return t("transfer.fund.error.willBeSent");
    case "platform.signer_unavailable":
      return t("transfer.fund.error.signerUnavailable");
    case "platform.context_unavailable":
      return t("transfer.fund.error.contextUnavailable");
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
  /** An add's call has been pending for a second. */
  readonly preparing = new Preparing(() => this.changed());
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
        if (request.kind === "add") this.preparing.start();
        this.changed();
        return request.kind === "add" ? this.api.platformFund(this.walletId, { kind: "duffs", duffs: request.duffs }, g.id) : this.api.resumePlatformFund(this.walletId, request.txid, g.id);
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
      this.preparing.stop();
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
