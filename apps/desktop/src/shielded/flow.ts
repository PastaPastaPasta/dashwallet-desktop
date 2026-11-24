// Shielded funds (0.1 release plan; lane E1's contract v41 §2.13): the moves in and out of the
// wallet's shielded pool. Add to Shielded (Core → Shielded), Send (Shielded → Shielded) and Move to
// Dash wallet (Shielded → Core) share one flow: an amount (and for a send an address and a memo),
// the quote's confirm step, the call under the quote's `platformOp` grant (U-05's GrantModel), its
// progress (proving a spend takes a while) and its outcome. A lock that was not applied is finished
// with `resume`. One flow per wallet, kept while the app runs, so the sheet can close while the call
// runs and reopen to its result (as withdraw does).

import { isAppError, type Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { ShieldedSpendOutcome } from "../api/generated/ShieldedSpendOutcome";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { SHIELDED_CODES } from "../api/pending";
import { RESCAN_TEXT } from "../app/rescan";
import { platformOp } from "../dashpay/topup";
import { t } from "../i18n";
import { failureText, Model } from "../onboarding/model";
import { moveErrorText } from "../platform/move";
import { SHIELDED_SYNC_COPY, syncingRefusal } from "./syncing";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type ShieldedApi = Pick<
  Backend,
  "shieldFromCore" | "resumeShield" | "shieldedSend" | "unshieldToCore" | "unshieldToPlatform" | "platformAddressesSync" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

/** What a sheet is for: the moves, and finishing a lock that was not applied. `toPlatform` moves to the
 * wallet's own Platform receive address. */
export type ShieldedKind = "add" | "send" | "out" | "toPlatform" | "resume";

/** What the confirm step quotes and sends. Credits are decimal strings, a duff is 1000 of them. */
export type ShieldedRequest =
  | { kind: "add"; duffs: number }
  | { kind: "send"; credits: string; to: string; memo: string | null }
  | { kind: "out"; credits: string }
  | { kind: "toPlatform"; credits: string }
  | { kind: "resume"; txid: string };

export type ShieldedStep = "form" | "confirm" | "running" | "done" | "refused";

export type ShieldedResult = { kind: "shield"; outcome: ShieldOutcome } | { kind: "spend"; outcome: ShieldedSpendOutcome };

export const SHIELDED_COPY = {
  card: {
    get inactive() {
      return t("transfer.shielded.card.inactive");
    },
    get finish() {
      return t("transfer.shielded.card.finish");
    },
    /** A lock that may already be applied: the same outpoint is read again, never a new lock. */
    get checkAgain() {
      return t("transfer.shielded.card.checkAgain");
    },
    why: {
      get empty() {
        return t("transfer.shielded.card.why.empty");
      },
      get busy() {
        return t("transfer.shielded.card.why.busy");
      },
    },
    unfinished: (amount: string) => t("transfer.shielded.card.unfinished", { amount }),
  },
  titles: {
    get add() {
      return t("transfer.shielded.titles.add");
    },
    get send() {
      return t("transfer.shielded.titles.send");
    },
    get receive() {
      return t("transfer.shielded.titles.receive");
    },
  },
  form: {
    why: {
      get amount() {
        return t("transfer.shielded.form.why.amount");
      },
      get to() {
        return t("transfer.shielded.form.why.to");
      },
      get memo() {
        return t("transfer.shielded.form.why.memo");
      },
    },
    get add() {
      return t("transfer.shielded.form.add");
    },
    get send() {
      return t("transfer.shielded.form.send");
    },
    get out() {
      return t("transfer.shielded.form.out");
    },
    get toPlatform() {
      return t("transfer.shielded.form.toPlatform");
    },
    get to() {
      return t("transfer.shielded.form.to");
    },
    get memo() {
      return t("transfer.shielded.form.memo");
    },
    get memoHint() {
      return t("transfer.shielded.form.memoHint");
    },
    available: (amount: string) => t("transfer.shielded.form.available", { amount }),
    get invalidAmount() {
      return t("transfer.shielded.form.invalidAmount");
    },
    get next() {
      return t("transfer.shielded.form.next");
    },
    get cancel() {
      return t("common.cancel");
    },
  },
  confirm: {
    get quoting() {
      return t("transfer.shielded.confirm.quoting");
    },
    get quoteFailed() {
      return t("transfer.shielded.confirm.quoteFailed");
    },
    get amount() {
      return t("transfer.shielded.confirm.amount");
    },
    get fee() {
      return t("transfer.shielded.confirm.fee");
    },
    get coreFee() {
      return t("transfer.shielded.confirm.coreFee");
    },
    get poolFee() {
      return t("transfer.shielded.confirm.poolFee");
    },
    poolFeeFixed: (amount: string) => t("transfer.shielded.confirm.poolFeeFixed", { amount }),
    get total() {
      return t("transfer.shielded.confirm.total");
    },
    get totalFromWallet() {
      return t("transfer.shielded.confirm.totalFromWallet");
    },
    get lock() {
      return t("transfer.shielded.confirm.lock");
    },
    get gain() {
      return t("transfer.shielded.confirm.gain");
    },
    get receive() {
      return t("transfer.shielded.confirm.receive");
    },
    get to() {
      return t("transfer.shielded.confirm.to");
    },
    get slow() {
      return t("transfer.shielded.confirm.slow");
    },
    get slowFirst() {
      return t("transfer.shielded.confirm.slowFirst");
    },
    get back() {
      return t("transfer.shielded.confirm.back");
    },
    get go() {
      return t("transfer.shielded.confirm.go");
    },
  },
  running: {
    get title() {
      return t("transfer.shielded.running.title");
    },
    get prove() {
      return t("transfer.shielded.running.prove");
    },
    get lock() {
      return t("transfer.shielded.running.lock");
    },
  },
  done: {
    add: {
      get title() {
        return t("transfer.shielded.done.add.title");
      },
      text: (balance: string) => t("transfer.shielded.done.add.text", { balance }),
    },
    waiting: {
      get title() {
        return t("transfer.shielded.done.waiting.title");
      },
      get text() {
        return t("transfer.shielded.done.waiting.text");
      },
    },
    checking: {
      get title() {
        return t("transfer.shielded.done.checking.title");
      },
      get text() {
        return t("transfer.shielded.done.checking.text");
      },
      get row() {
        return t("transfer.shielded.done.checking.row");
      },
    },
    notApplied: {
      get title() {
        return t("transfer.shielded.done.notApplied.title");
      },
      text: (why: string) => t("transfer.shielded.done.notApplied.text", { why }),
    },
    send: {
      get title() {
        return t("transfer.shielded.done.send.title");
      },
      text: (amount: string) => t("transfer.shielded.done.send.text", { amount }),
    },
    /** The spend went out and its result is not confirmed: pending, never failed, never offered for a resend. */
    unconfirmed: {
      get title() {
        return t("transfer.shielded.done.unconfirmed.title");
      },
      get text() {
        return t("transfer.shielded.done.unconfirmed.text");
      },
    },
    out: {
      get title() {
        return t("transfer.shielded.done.out.title");
      },
      text: (amount: string) => t("transfer.shielded.done.out.text", { amount }),
    },
    toPlatform: {
      get title() {
        return t("transfer.shielded.done.out.title");
      },
      text: (addr: string) => t("transfer.shielded.done.toPlatform.text", { addr }),
    },
  },
  /** The confirmation a send or move from a wallet with an unrecorded spend needs (DEC-1057). */
  unrecorded: {
    get title() {
      return t("transfer.shielded.unrecorded.title");
    },
    get text() {
      return t("transfer.shielded.unrecorded.text");
    },
    go: {
      get send() {
        return t("transfer.shielded.unrecorded.go.send");
      },
      get out() {
        return t("transfer.shielded.unrecorded.go.out");
      },
    },
    get back() {
      return t("transfer.shielded.unrecorded.back");
    },
    /** The engine's refusal when no confirmation came with the call. */
    get refused() {
      return t("transfer.shielded.unrecorded.refused");
    },
  },
  /** A shield's or a move of mixed coins' lock that signed and may be out (`platform.broadcast_unknown`):
   * its step refuses the next one until Tools ▸ Repair settles it. */
  get fundingUnknown() {
    return t("transfer.shielded.fundingUnknown");
  },
  get close() {
    return t("transfer.shielded.close");
  },
  get show() {
    return t("transfer.shielded.show");
  },
  get unseen() {
    return t("transfer.shielded.unseen");
  },
  get unseenSend() {
    return t("transfer.shielded.unseenSend");
  },
  receive: {
    get text() {
      return t("transfer.shielded.receive.text");
    },
    get copy() {
      return t("transfer.shielded.receive.copy");
    },
    get notSynced() {
      return SHIELDED_SYNC_COPY.syncing;
    },
    get loading() {
      return t("transfer.shielded.receive.loading");
    },
  },
};

/** An error as dw-app sends it: its code (the generated `ErrorCode` lists the shielded ones) and data. */
function errorLike(e: unknown): { code: string; data: Record<string, unknown> | null } | null {
  if (!isAppError(e)) return null;
  const data = e.data;
  return { code: e.code, data: typeof data === "object" && data !== null ? (data as Record<string, unknown>) : null };
}

/** Whether `e` is the app error `code`. */
export const isCode = (e: unknown, code: string): boolean => errorLike(e)?.code === code;

/** A shielded refusal, by code. `credits` shows a credit figure as Dash, `duffs` a duff figure. */
export function shieldedErrorText(e: unknown, credits: (c: string) => string, duffs: (d: number) => string, kind?: ShieldedKind, syncing = false): string {
  const err = errorLike(e);
  if (!err) return failureText(e);
  const d = err.data;
  if (syncingRefusal(err.code, syncing)) return SHIELDED_SYNC_COPY.syncing;
  switch (err.code) {
    case SHIELDED_CODES.notActive:
      return SHIELDED_COPY.card.inactive;
    case SHIELDED_CODES.poolTooSmall:
      return t("transfer.shielded.error.poolTooSmall");
    case SHIELDED_CODES.invalidAddress:
      return t("transfer.shielded.error.invalidAddress");
    case SHIELDED_CODES.insufficientFunds:
      return d?.kind === "credits" ? t("transfer.shielded.error.insufficient", { needed: credits(String(d.needed)), available: credits(String(d.available)) }) : t("transfer.shielded.error.insufficientShort");
    case SHIELDED_CODES.fundingInsufficient:
      return d?.kind === "duffs" ? t("transfer.shielded.error.fundingInsufficient", { needed: duffs(Number(d.needed)), available: duffs(Number(d.available)) }) : t("transfer.shielded.error.fundingInsufficientShort");
    case SHIELDED_CODES.belowMinimum:
      // The engine's minimum is in duffs for a shield, in credits for a spend.
      if (d?.kind !== "number") return t("transfer.shielded.error.largerAmount");
      return kind === "add" ? t("transfer.shielded.error.belowMinimumAdd", { amount: duffs(Number(d.value)) }) : t("transfer.shielded.error.belowMinimum", { amount: credits(String(d.value)) });
    case SHIELDED_CODES.coinsLocked:
      return t("transfer.shielded.error.coinsLocked");
    case SHIELDED_CODES.unrecordedSpend:
      return SHIELDED_COPY.unrecorded.refused;
    case SHIELDED_CODES.tooManyInputs:
      return t("transfer.shielded.error.tooManyInputs");
    case SHIELDED_CODES.shieldPending:
      return t("transfer.shielded.error.pending");
    case "platform.context_unavailable":
      return t("transfer.shielded.error.contextUnavailable");
    case "rescan_interrupted":
      return RESCAN_TEXT.interrupted;
    case "platform.trust_mismatch":
      return t("transfer.shielded.error.trustMismatch");
    case "platform.feature_off":
      return t("transfer.shielded.error.featureOff");
    case "platform.broadcast_unknown":
      if (kind === "add") return SHIELDED_COPY.fundingUnknown;
      return kind === "toPlatform" ? t("transfer.shielded.error.toPlatformUnknown") : moveErrorText(e, credits);
    default:
      return moveErrorText(e, credits);
  }
}

/** Back is offered when another amount or address may pass. */
export const shieldedCanChange = (e: unknown): boolean =>
  [SHIELDED_CODES.invalidAddress, SHIELDED_CODES.insufficientFunds, SHIELDED_CODES.fundingInsufficient, SHIELDED_CODES.belowMinimum, "platform.grant_exceeded", "invalid_argument", SHIELDED_CODES.unrecordedSpend].some((c) => isCode(e, c));

const settledListeners = new Set<() => void>();

/** Called whenever a shielded call that ran has ended (events.ts re-reads `["dashpay"]` from it, so
 * the balances and Activity follow even when Home is not shown). Returns the unsubscribe. */
export function onShieldedSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

/** Tells the listeners a shielded call ran (a move of mixed coins to Shielded too). */
export function shieldedSettled() {
  for (const f of settledListeners) queueMicrotask(f);
}

export class ShieldedFlow extends Model {
  /** The sheet is shown. */
  open = false;
  kind: ShieldedKind = "add";
  step: ShieldedStep = "form";
  /** The form's fields as typed (the amount in the display unit). */
  text = "";
  to = "";
  memo = "";
  request: ShieldedRequest | null = null;
  result: ShieldedResult | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: ShieldedApi,
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

  /** Opens the sheet for `kind`: at a fresh form, or where a running or unseen call is. */
  begin(kind: ShieldedKind) {
    this.open = true;
    if (!this.running && this.step !== "done" && this.step !== "refused") {
      this.kind = kind;
      this.step = "form";
      this.text = "";
      this.to = "";
      this.memo = "";
      this.request = null;
      this.result = null;
      this.error = null;
    }
    this.changed();
  }

  /** Opens a fresh send to `to` for `text` (Pay's Shielded address); a running or unseen call shows instead. */
  sendTo(to: string, text: string) {
    this.begin("send");
    if (this.running || this.step !== "form") return;
    this.to = to;
    this.text = text;
    this.changed();
  }

  setText(text: string) {
    this.text = text;
    this.changed();
  }

  setTo(to: string) {
    this.to = to;
    this.changed();
  }

  setMemo(memo: string) {
    this.memo = memo;
    this.changed();
  }

  /** Form → confirm for a request the screen has checked. */
  next(request: ShieldedRequest) {
    this.request = request;
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
    this.kind = "resume";
    this.step = "form";
    this.result = null;
    this.error = null;
    this.request = { kind: "resume", txid };
    this.changed();
    await this.run({ maxDuffs: 1, maxCredits: "0" });
  }

  /** Confirm: the quote's grant, then the call (one at a time). `confirmUnrecorded`: the user
   * confirmed going on while the wallet holds a spend whose record was lost (DEC-1057). */
  async run(grant: GrantRequest, confirmUnrecorded = false): Promise<void> {
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
        return this.call(request, g.id, confirmUnrecorded);
      });
      if (got) {
        this.result = got.value;
        this.step = "done";
        if (request.kind === "toPlatform" && got.value.kind === "spend" && got.value.outcome.state === "done") this.readPlatform();
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
      if (ran) shieldedSettled();
      this.changed();
    }
  }

  /** The Platform address a done move credited is read now, not at the next pass: Home's total
   * follows it. A failed read changes nothing of the move. */
  private readPlatform() {
    this.api.platformAddressesSync(this.walletId).then(shieldedSettled, () => undefined);
  }

  private async call(r: ShieldedRequest, grant: string, confirmUnrecorded: boolean): Promise<ShieldedResult> {
    switch (r.kind) {
      case "add":
        return { kind: "shield", outcome: await this.api.shieldFromCore(this.walletId, { kind: "duffs", duffs: r.duffs }, grant) };
      case "resume":
        return { kind: "shield", outcome: await this.api.resumeShield(this.walletId, r.txid, grant) };
      case "send":
        return { kind: "spend", outcome: await this.api.shieldedSend(this.walletId, r.to, r.credits, r.memo, grant, confirmUnrecorded) };
      case "out":
        return { kind: "spend", outcome: await this.api.unshieldToCore(this.walletId, r.credits, null, grant, confirmUnrecorded) };
      case "toPlatform":
        return { kind: "spend", outcome: await this.api.unshieldToPlatform(this.walletId, r.credits, null, grant, confirmUnrecorded) };
    }
  }
}

const flows = new Map<string, ShieldedFlow>();

/** The wallet's shielded flow, created on first use and kept while the app runs. */
export function shieldedFor(api: ShieldedApi, walletId: string): ShieldedFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new ShieldedFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetShielded() {
  flows.clear();
}
