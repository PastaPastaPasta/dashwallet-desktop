// "Move to Dash wallet" (0.1 release plan; lane E2's contract §2.12): everything on the wallet's
// Platform addresses goes to its own Core receive address. There is no form: the quote
// (`platform_move_to_core_quote`) names what is taken, the fee and what arrives, and Confirm runs
// `platform_move_to_core` under the quote's `platformOp` grant (U-05's GrantModel). One flow per
// wallet, kept while the app runs, so the sheet can close while the call runs and reopen to its
// result (as withdraw does).

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { platformOp } from "../dashpay/topup";
import { t } from "../i18n";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type PlatformMoveApi = Pick<Backend, "platformMoveToCore" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const MOVE_COPY = {
  card: {
    /** What one move cannot take (below the minimum input, beyond the inputs one transition takes, or the fee reserve): it stays on Platform. */
    dust: (amount: string) => t("transfer.move.dust", { amount }),
  },
  get message() {
    return t("transfer.move.message");
  },
  rows: {
    get taken() {
      return t("transfer.move.rows.taken");
    },
    get fee() {
      return t("transfer.move.rows.fee");
    },
    get receive() {
      return t("transfer.move.rows.receive");
    },
    get to() {
      return t("transfer.move.rows.to");
    },
  },
  get quoting() {
    return t("transfer.move.quoting");
  },
  get quoteFailed() {
    return t("transfer.move.quoteFailed");
  },
  get back() {
    return t("transfer.move.back");
  },
  get go() {
    return t("transfer.move.go");
  },
  get cancel() {
    return t("common.cancel");
  },
  running: {
    get title() {
      return t("transfer.move.running.title");
    },
    get text() {
      return t("transfer.move.running.text");
    },
  },
  done: {
    get title() {
      return t("transfer.move.done.title");
    },
    text: (amount: string, fee: string | null) => (fee === null ? t("transfer.move.done.text", { amount }) : t("transfer.move.done.textWithFee", { amount, fee })),
    /** The engine proved the debit but not what arrives. */
    debitOnly: (taken: string) => t("transfer.move.done.debitOnly", { taken }),
  },
  pending: {
    get title() {
      return t("transfer.move.pending.title");
    },
    text: (amount: string) => t("transfer.move.pending.text", { amount }),
    debitOnly: (taken: string) => t("transfer.move.pending.debitOnly", { taken }),
  },
  /** A move that cannot execute: the top-up it exits, or an earlier exit, took its credits. */
  failed: {
    get title() {
      return t("transfer.move.failed.title");
    },
    get text() {
      return t("transfer.move.failed.text");
    },
  },
  /** A move the engine superseded with a retry: not a loss, and nothing to wait for. */
  replaced: {
    get title() {
      return t("transfer.move.replaced.title");
    },
    get text() {
      return t("transfer.move.replaced.text");
    },
  },
  get unseen() {
    return t("transfer.move.unseen");
  },
  waiting: {
    get text() {
      return t("transfer.move.waiting.text");
    },
    get held() {
      return t("transfer.move.waiting.held");
    },
    /** A payment (not a move) from Platform is unsettled: Move waits until it settles. */
    get sendHeld() {
      return t("transfer.move.waiting.sendHeld");
    },
    /** Move refused beside a held payment: that payment's own retry (in Pay) is the way out. */
    get heldSend() {
      return t("transfer.move.waiting.heldSend");
    },
    get action() {
      return t("transfer.move.waiting.action");
    },
  },
  /** Before Move is offered, Activity must say no move is unfinished (after a relaunch the flow remembers nothing). */
  check: {
    get loading() {
      return t("transfer.move.check.loading");
    },
    get failed() {
      return t("transfer.move.check.failed");
    },
    get again() {
      return t("transfer.move.check.again");
    },
  },
  get show() {
    return t("transfer.move.show");
  },
  get close() {
    return t("transfer.move.close");
  },
};

/** What holds Move back: the Activity read has not answered (`checking`) or failed (`failed`), or it names
 * a move Platform has not confirmed (`pending`). `null`: Move may be offered. */
export type MoveBlock = "checking" | "failed" | "pending";

/** The block from Activity's read state: `known` is false while it is loading or failed. */
export function moveBlock(waiting: boolean, read: { isPending: boolean; isError: boolean }): MoveBlock | null {
  if (waiting) return "pending";
  if (read.isError) return "failed";
  if (read.isPending) return "checking";
  return null;
}

export type MoveStep = "confirm" | "running" | "done" | "refused";

/** An `invalid_argument` refusal says what is wrong in words a person can act on (for example
 * "consolidate coins first"): show that detail, not the code. */
export function invalidArgumentText(e: { message: string; data: { kind: string; reason?: string } | null }): string {
  const detail = e.data?.kind === "reason" && e.data.reason ? e.data.reason : e.message;
  return detail.trim() === "" ? t("transfer.move.notAccepted") : /[.!?]$/.test(detail) ? detail : `${detail}.`;
}

/** A move's refusal, by code. `credits` shows a credit figure as Dash. */
export function moveErrorText(e: unknown, credits: (c: string) => string, activity?: readonly PlatformActivity[] | null): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? t("transfer.move.error.insufficient", { needed: credits(e.data.needed), available: credits(e.data.available) }) : t("transfer.move.error.insufficientFee");
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? t("transfer.move.error.outOfRange", { min: credits(e.data.min), max: credits(e.data.max) }) : t("transfer.move.error.outOfRangeAmount");
    case "credits.move_pending": {
      const id = e.data?.kind === "artifact" ? e.data.artifact : null;
      return activity?.find((a) => a.id === id)?.kind === "send" ? MOVE_COPY.waiting.heldSend : MOVE_COPY.waiting.held;
    }
    case "platform.broadcast_unknown":
      return t("transfer.move.error.broadcastUnknown");
    case "platform.context_unavailable":
      return t("transfer.move.error.contextUnavailable");
    case "platform.trust_mismatch":
      return t("transfer.move.error.trustMismatch");
    case "platform.unavailable":
    case "platform.timeout":
      return t("transfer.move.error.unavailable");
    case "platform.signer_unavailable":
      return t("transfer.move.error.signerUnavailable");
    case "platform.grant_exceeded":
      return t("transfer.move.error.grantExceeded");
    case "credits.quote_changed":
      return t("transfer.move.error.quoteChanged");
    case "platform.feature_off":
      return t("transfer.move.error.featureOff");
    case "invalid_argument":
      return invalidArgumentText(e);
    default:
      return t("transfer.move.error.stopped", { code: e.code });
  }
}

/** Back is offered when a new quote may pass. */
export const moveCanRetry = (e: unknown) => hasCode(e, "platform.grant_exceeded") || hasCode(e, "platform.insufficient_credits") || hasCode(e, "credits.quote_changed");

/** A move, send or top-up from Platform that Platform has not answered (`pending`) or that may still
 * execute (`mayLand`: also a `failed` one by height). They share one hold on the wallet's Platform addresses. */
export const isHeldOp = (a: PlatformActivity): boolean => (a.kind === "moveToCore" || a.kind === "send" || a.kind === "toIdentity") && (a.state === "pending" || a.mayLand);

/** Whether a move or send of this wallet holds the Platform addresses: recorded `pending` or able to
 * execute still (it settles within minutes: the engine's `platform_activity` settles it). While one
 * does, the engine refuses any other move or send that is not its exact retry. */
export const hasPendingMove = (activity: readonly PlatformActivity[] | null | undefined): boolean => !!activity?.some(isHeldOp);

/** Whether a move or send is recorded `pending`: Platform has not answered, so not even its retry
 * is taken yet. */
export const hasUnansweredOp = (activity: readonly PlatformActivity[] | null | undefined): boolean =>
  !!activity?.some((a) => (a.kind === "moveToCore" || a.kind === "send" || a.kind === "toIdentity") && a.state === "pending");

/** Whether a payment (a `send`, not a move) holds the Platform addresses and no move does: the Move card says so. */
export const sendHolds = (activity: readonly PlatformActivity[] | null | undefined): boolean =>
  !!activity?.some((a) => a.kind === "send" && isHeldOp(a)) && !activity.some((a) => a.kind === "moveToCore" && isHeldOp(a));

const settledListeners = new Set<() => void>();

/** Called whenever a move that ran has ended (events.ts re-reads `["dashpay"]` from it, so the
 * balance and Activity follow even when Home is not shown). Returns the unsubscribe. */
export function onMoveSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

/** Tells the listeners a move or send that ran has ended. */
export function onMoveSettledNotify() {
  for (const f of settledListeners) queueMicrotask(f);
}

export class PlatformMoveFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: MoveStep = "confirm";
  outcome: MoveToCoreOutcome | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: PlatformMoveApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** A result that came while the sheet was hidden, not yet seen. */
  get unseen(): boolean {
    return !this.open && !this.running && !this.pending && (this.step === "done" || this.step === "refused");
  }

  /** The last move's result is `pending`: Platform has not said whether it went through. It stays
   * (closing the sheet does not drop it) until Activity shows the move settled (`settle`). */
  get pending(): boolean {
    return this.step === "done" && this.outcome?.state === "pending";
  }

  /** Drops a retained `pending` result once `activity` has its move as `done` or `failed`. An open
   * sheet keeps showing it (a new quote would hide that the move went through): the caller asks
   * again once it is closed. */
  settle(activity: readonly PlatformActivity[] | null | undefined) {
    const id = this.outcome?.id;
    if (!this.pending || !activity || this.running || this.open) return;
    const row = activity.find((a) => a.id === id);
    if (row && row.state !== "pending") {
      this.step = "confirm";
      this.outcome = null;
      this.changed();
    }
  }

  /** Opens the sheet: at the quote, or where a running or unseen call is. */
  begin() {
    this.open = true;
    if (!this.running && this.step !== "done" && this.step !== "refused") {
      this.step = "confirm";
      this.outcome = null;
      this.error = null;
    }
    this.changed();
  }

  /** Back to a fresh quote after a refusal. */
  back() {
    if (this.running) return;
    this.step = "confirm";
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Hides the sheet; a running call goes on and its result waits. A result seen is dropped,
   * except a `pending` one, which stays until Activity shows the move settled. */
  close() {
    this.open = false;
    if (!this.running) {
      if ((this.step === "done" && !this.pending) || this.step === "refused") this.step = "confirm";
      this.grants.cancel();
    }
    this.changed();
  }

  /** Confirm & move: the quote's grant, then `platformMoveToCore` for everything to the quote's `to`, one
   * of the wallet's own receiving addresses, as the exit of the quote's `exitOf` (one call at a time). */
  async move(grant: GrantRequest, to: string, exitOf: string | null): Promise<void> {
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
        return this.api.platformMoveToCore(this.walletId, { kind: "all" }, to, exitOf, g.id);
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
      if (ran) onMoveSettledNotify();
      this.changed();
    }
  }
}

const flows = new Map<string, PlatformMoveFlow>();

/** The wallet's move flow, created on first use and kept while the app runs. */
export function moveFor(api: PlatformMoveApi, walletId: string): PlatformMoveFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new PlatformMoveFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetPlatformMoves() {
  flows.clear();
}

/** The withdrawal excludes the fee reserve included in the grant cap. */
export const plannedWithdrawalCredits = (q: { credits: string; feeCredits: string }): string => String(BigInt(q.credits) - BigInt(q.feeCredits));
