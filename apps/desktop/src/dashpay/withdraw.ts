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
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { messageTable, t } from "../i18n";
import { creditCount, creditsToDuffs, PROFILE_COPY } from "./profile";
import { withTable } from "./status";
import { platformOp } from "./topup";
import { platformHoldText } from "../api/platformHold";

export type WithdrawApi = Pick<Backend, "creditsWithdraw" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

const wd = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(prefix ? `dashpay.withdraw.${prefix}` : "dashpay.withdraw", keys);

export const WITHDRAW_COPY = withTable(wd("", ["title", "action", "unseen", "show", "ok", "close"]), {
  form: withTable(
    wd("form", ["intro", "amount", "max", "maxLine", "to", "toHint", "invalidAmount", "first", "noCosts", "invalidTo", "platformTo", "unconfirmed", "next", "cancel"]),
    { amountUnit: (unit: string) => t("dashpay.withdraw.form.amountUnit", { unit }) },
  ),
  confirm: wd("confirm", ["message", "withdraw", "fee", "total", "receive", "to", "quoting", "back", "go"]),
  refused: {
    /** Followed by the document id. */
    get document() {
      return t("dashpay.withdraw.document");
    },
  },
  running: wd("running", ["title", "text"]),
  done: withTable(wd("done", ["title", "landed"]), {
    text: (amount: string, to: string) => t("dashpay.withdraw.done.text", { amount, to }),
    balance: (balance: string) => t("dashpay.withdraw.done.balance", { balance }),
  }),
  recent: withTable(wd("recent", ["title", "unconfirmed", "unconfirmedText"]), {
    row: (amount: string) => t("dashpay.withdraw.recent.row", { amount }),
    status: wd("recent.status", ["pending", "complete", "failed", "unknown"]),
  }),
});

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

/** The withdrawals My Profile lists: the newest `RECENT_SHOWN` (the engine reads them newest first). */
const listed = (list: Withdrawal[] | undefined) => (list ?? []).slice(0, RECENT_SHOWN);

/** A row's amount: the payout, else the credits, else `null`: unknown (D25 n5). */
function rowAmount(w: Withdrawal, shown: number | null, amount: (duffs: number) => string): string | null {
  if (shown !== null) return amount(shown);
  if (w.credits === null) return null;
  return PROFILE_COPY.credits.creditsOnly(creditCount(w.credits));
}

const rowTitle = (amount: string | null) => (amount === null ? t("dashpay.withdraw.recent.rowUnknown") : WITHDRAW_COPY.recent.row(amount));

/** My Profile's rows. */
export function withdrawalRows(list: Withdrawal[] | undefined, costs: Pick<CostTable, "creditsPerDuff"> | undefined, amount: (duffs: number) => string): WithdrawalRow[] {
  return listed(list).map((w) => {
    const s = statusText(w.status);
    const shown = w.expectedDuffs ?? (costs && w.credits !== null ? creditsToDuffs(w.credits, costs) : null);
    const at = w.updatedAt ?? w.createdAt;
    return {
      id: w.id,
      title: rowTitle(rowAmount(w, shown, amount)),
      status: s.text,
      pending: s.pending,
      to: w.to,
      at: at === null ? null : Math.floor(at / 1000),
    };
  });
}

/** D23: the withdrawal whose outcome is unknown, while Platform does not list it. */
export const unconfirmedIn = (list: Withdrawal[] | undefined, id: string | null) => (id !== null && !(list ?? []).some((w) => w.id === id) ? id : null);

/** D11: read again every minute while a row is pending or a withdrawal unconfirmed (D23), else
 * only on signals. */
export const pollWhilePending = (list: Withdrawal[] | undefined, unconfirmed: string | null = null) =>
  unconfirmedIn(list, unconfirmed) !== null || listed(list).some((w) => statusText(w.status).pending) ? PENDING_POLL_MS : false;

/** The withdrawal document an unknown outcome names (D7). */
export const unknownDocument = (e: unknown) => (hasCode(e, "platform.broadcast_unknown") && e.data?.kind === "artifact" ? e.data.artifact : null);

/** The document a refusal names: an unknown outcome's, or the earlier withdrawal that went through (D24). */
export const refusalDocument = (e: unknown) =>
  unknownDocument(e) ?? (hasCode(e, "credits.withdrawal_landed") && e.data?.kind === "artifact" ? e.data.artifact : null);

/** A withdrawal's refusal, by code (§3's table). `credits` shows a credit figure as Dash. */
export function withdrawErrorText(e: unknown, credits: (c: string) => string): string {
  const held = platformHoldText(e);
  if (held) return held;
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? t("dashpay.withdraw.error.range", { min: credits(e.data.min), max: credits(e.data.max) }) : t("dashpay.withdraw.error.rangeUnknown");
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? t("dashpay.withdraw.error.credits", { needed: credits(e.data.needed), available: credits(e.data.available) }) : t("dashpay.withdraw.error.creditsShort");
    case "identity.keys_missing":
      return t("dashpay.withdraw.error.keysMissing");
    case "invalid_argument":
      return t("dashpay.withdraw.error.address");
    case "platform.broadcast_unknown":
      return t("dashpay.withdraw.error.maybeSent");
    case "credits.withdrawal_landed":
      return t("dashpay.withdraw.error.landed");
    case "platform.context_unavailable":
      return t("dashpay.withdraw.error.context");
    case "platform.trust_mismatch":
      return t("dashpay.withdraw.error.trust");
    case "platform.unavailable":
    case "platform.timeout":
      return t("dashpay.withdraw.error.unavailable");
    case "platform.signer_unavailable":
      return t("dashpay.withdraw.error.signer");
    case "platform.grant_exceeded":
      return t("dashpay.withdraw.error.grantExceeded");
    default:
      return t("dashpay.withdraw.error.stoppedWith", { code: e.code });
  }
}

/** Back is offered when another amount or address may pass. */
export const amountCanChange = (e: unknown) =>
  hasCode(e, "credits.withdrawal_out_of_range") || hasCode(e, "platform.insufficient_credits") || hasCode(e, "platform.grant_exceeded") || hasCode(e, "invalid_argument");

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
  /** D23: the last withdrawal whose outcome is unknown, kept while the app runs. */
  unconfirmed: { identity: string; id: string } | null = null;
  readonly grants: GrantModel;

  constructor(
    private readonly api: WithdrawApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** D23: `identity`'s withdrawal whose outcome is unknown, its document id. */
  unconfirmedFor(identity: string): string | null {
    return this.unconfirmed?.identity === identity ? this.unconfirmed.id : null;
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
        // D24: the engine signs only once the unconfirmed one can no longer execute, or at its nonce.
        if (this.unconfirmed?.identity === identity) this.unconfirmed = null;
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
      const id = unknownDocument(e);
      if (id !== null) this.unconfirmed = { identity, id };
      // D24: the unconfirmed one went through, and no later one is pending.
      else if (hasCode(e, "credits.withdrawal_landed") && this.unconfirmed?.identity === identity) this.unconfirmed = null;
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
