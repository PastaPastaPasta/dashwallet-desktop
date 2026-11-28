// "Move to Dash wallet" (0.1 release plan; lane E2's contract §2.12): everything on the wallet's
// Platform addresses goes to its own Core receive address. There is no form: the quote
// (`platform_move_to_core_quote`) names what is taken, the fee and what arrives, and Confirm runs
// `platform_move_to_core` under the quote's `platformOp` grant (U-05's GrantModel). One flow per
// wallet, kept while the app runs, so the sheet can close while the call runs and reopen to its
// result (as withdraw does).

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { MoveToCoreQuote } from "../api/generated/MoveToCoreQuote";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { platformOp } from "../dashpay/topup";
import { contactName, shortAddress } from "../format/tx";
import { t } from "../i18n";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { isAddressSendsOff, MAINNET_OFF_COPY } from "./mainnetOff";
import { platformHoldText } from "../api/platformHold";

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
    /** `debitOnly` when nothing says which member of the move's group went through. */
    debitUnknown: (taken: string) => t("transfer.move.done.debitUnknown", { taken }),
  },
  /** The move's group settled with no evidence of which member went through. */
  settledUnknown: {
    get title() {
      return t("transfer.status.settledUnknown.title");
    },
    get text() {
      return t("transfer.status.settledUnknown.text");
    },
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
  /** The move is the exit of a payment from Platform that may still go through: only one of the two goes
   * through. `recipient`: the payment's contact, else its shortened address. */
  exitSend: {
    line: (amount: string, recipient: string) => t("transfer.move.exitSend.line", { amount, recipient }),
    done: (recipient: string) => t("transfer.move.exitSend.done", { recipient }),
    lost: (recipient: string) => t("transfer.move.exitSend.lost", { recipient }),
  },
  /** The move is the exit of a cancel that may still go through, beside the move or payment it cancels:
   * only one of the three goes through. `recipient`: the cancelled payment's. */
  exitCancel: {
    line: {
      move: (amount: string) => t("transfer.move.exitCancel.line.move", { amount }),
      payment: (amount: string, recipient: string) => t("transfer.move.exitCancel.line.payment", { amount, recipient }),
    },
    done: {
      get move() {
        return t("transfer.move.exitCancel.done.move");
      },
      payment: (recipient: string) => t("transfer.move.exitCancel.done.payment", { recipient }),
    },
    lostToCancel: {
      get move() {
        return t("transfer.move.exitCancel.lostToCancel.move");
      },
      payment: (recipient: string) => t("transfer.move.exitCancel.lostToCancel.payment", { recipient }),
    },
    get lostToMove() {
      return t("transfer.move.exitCancel.lostToMove");
    },
    lostToPayment: (recipient: string) => t("transfer.move.exitCancel.lostToPayment", { recipient }),
  },
  /** The move is the exit of a move too small to cancel: only one of the two goes through. */
  exitMove: {
    line: (amount: string) => t("transfer.move.exitMove.line", { amount }),
    get lost() {
      return t("transfer.move.exitMove.lost");
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

/** Who a held payment from Platform pays, as its row shows it: the contact, else the shortened address. */
export const recipientOf = (a: PlatformActivity): string => contactName(a.contact) ?? (a.to ? shortAddress(a.to) : "");

/** What a move is the exit of (`MoveToCoreQuote.exitOf`), from Activity's rows: a held send, a held cancel
 * beside the move or contact payment it cancels (`held`), or a held move or contact payment too small to
 * cancel. `null` for a top-up's exit or a row Activity does not show. */
export type MoveExit = { kind: "send" | "move" | "payment"; row: PlatformActivity } | { kind: "cancel"; of: "move" | "payment"; row: PlatformActivity; held: PlatformActivity | null };

export function moveExit(activity: readonly PlatformActivity[] | null | undefined, id: string | null | undefined): MoveExit | null {
  const row = id ? activity?.find((a) => a.id === id) : undefined;
  if (!row) return null;
  if (row.kind === "send") {
    if (!row.cancels) return { kind: "send", row };
    const held = activity?.find((a) => a.id === row.cancels) ?? null;
    return { kind: "cancel", of: held?.kind === "contactPayment" ? "payment" : "move", row, held };
  }
  if (row.kind === "moveToCore") return { kind: "move", row };
  if (row.kind === "contactPayment") return { kind: "payment", row };
  return null;
}

/** The record an Activity row is "Move to Dash wallet" out of: the engine keeps that link in the exit's id
 * (`exit-of-<id>-at-…`). `null` for every other row. */
export function exitLinkOf(id: string): string | null {
  const rest = id.startsWith("exit-of-") ? id.slice("exit-of-".length) : null;
  const at = rest?.lastIndexOf("-at-") ?? -1;
  return rest && at > 0 ? rest.slice(0, at) : null;
}

/** Whether the quote names a held record (`exitOf`) that Activity's rows do not show yet: Confirm cannot say
 * what the move replaces, so Go waits. A top-up's exit has its row, though `moveExit` gives it no line. */
export const exitUnresolved = (activity: readonly PlatformActivity[] | null | undefined, exitOf: string | null | undefined): boolean => !!exitOf && !activity?.some((a) => a.id === exitOf);

/** Confirm's line for the move out of `exit`: what may still go through, and that only one does. */
export function moveExitLine(exit: MoveExit, credits: (c: string) => string): string {
  const C = MOVE_COPY;
  const amount = credits(exit.row.credits);
  if (exit.kind === "cancel") return exit.of === "payment" ? C.exitCancel.line.payment(amount, exit.held ? recipientOf(exit.held) : "") : C.exitCancel.line.move(amount);
  return exit.kind === "move" ? C.exitMove.line(amount) : C.exitSend.line(amount, recipientOf(exit.row));
}

/** The outcome of the move out of `exit`: `lost` to the one that went through first, else done (`null`:
 * the plain move's text). */
function exitOutcomeText(exit: MoveExit, lost: boolean): string | null {
  const C = MOVE_COPY;
  const won = (row: PlatformActivity | null) => row?.state === "done" && row.winnerKnown !== false;
  if (exit.kind !== "cancel") {
    if (exit.kind === "move") return lost && won(exit.row) ? C.exitMove.lost : null;
    if (!lost) return C.exitSend.done(recipientOf(exit.row));
    return won(exit.row) ? C.exitSend.lost(recipientOf(exit.row)) : null;
  }
  const recipient = exit.held ? recipientOf(exit.held) : "";
  const payment = exit.of === "payment";
  if (!lost) return payment ? C.exitCancel.done.payment(recipient) : C.exitCancel.done.move;
  if (won(exit.held)) return payment ? C.exitCancel.lostToPayment(recipient) : C.exitCancel.lostToMove;
  if (won(exit.row)) return payment ? C.exitCancel.lostToCancel.payment(recipient) : C.exitCancel.lostToCancel.move;
  return null;
}

/** What the result sheet says of a move's outcome. `failed`: it failed and can no longer land.
 * `credits` and `amount` format credits and duffs. `exit`: what the move was the exit of; the documents
 * say which went through. */
export function moveOutcomeCopy(outcome: MoveToCoreOutcome, failed: boolean, credits: (c: string) => string, amount: (duffs: number) => string, exit: MoveExit | null = null): { title: string; text: string } {
  const C = MOVE_COPY;
  if (outcome.winnerKnown === false) return outcome.state === "done" ? { title: C.settledUnknown.title, text: C.done.debitUnknown(credits(outcome.credits)) } : C.settledUnknown;
  if (outcome.state === "replaced") return C.replaced;
  if (failed) return exit === null ? C.failed : { title: C.failed.title, text: exitOutcomeText(exit, true) ?? C.failed.text };
  const exited = outcome.state === "done" && exit !== null ? exitOutcomeText(exit, false) : null;
  if (exited !== null) return { title: C.done.title, text: exited };
  if (outcome.state === "done") return { title: C.done.title, text: outcome.expectedDuffs === null ? C.done.debitOnly(credits(outcome.credits)) : C.done.text(amount(outcome.expectedDuffs), outcome.feeCredits === null ? null : credits(outcome.feeCredits)) };
  return { title: C.pending.title, text: outcome.expectedDuffs === null ? C.pending.debitOnly(credits(outcome.credits)) : C.pending.text(amount(outcome.expectedDuffs)) };
}

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

/** Platform's structure pre-check refused a move, a send or a top-up before signing. Its consensus
 * error stays in the log. */
export function preflightText(code: "credits.rules_unsupported" | "credits.preflight_refused", earlier: "send" | "topUp" | "held" | null = null): string {
  const rules = code === "credits.rules_unsupported";
  if (earlier === "held") return rules ? t("platform.preflight.rulesUnsupportedHeld") : t("platform.preflight.refusedHeld");
  if (earlier === "send") return rules ? t("platform.preflight.rulesUnsupportedRetry") : t("platform.preflight.refusedRetry");
  if (earlier === "topUp") return rules ? t("platform.preflight.rulesUnsupportedHeldTopUp") : t("platform.preflight.refusedHeldTopUp");
  return rules ? t("platform.preflight.rulesUnsupported") : t("platform.preflight.refused");
}

/** Whether the earlier item a refused retry or exit named (`id`) may still have gone or go through. The
 * engine runs the pre-check for a named call only while that item holds, so it counts as held unless
 * Activity has since shown it failed for good: one that landed after the refusal changed the balance too. */
export const stillHeld = (activity: readonly PlatformActivity[] | null | undefined, id: string | null | undefined): boolean =>
  !!id && !activity?.some((a) => a.id === id && a.state === "failed" && !a.mayLand);

/** A move's refusal, by code. `credits` shows a credit figure as Dash. `exitOf`: the held top-up or payment
 * the move was the exit of; while it still holds, a pre-check refusal says it may still land. */
export function moveErrorText(e: unknown, credits: (c: string) => string, activity?: readonly PlatformActivity[] | null, exitOf?: string | null): string {
  const held = platformHoldText(e);
  if (held) return held;
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? t("transfer.move.error.insufficient", { needed: credits(e.data.needed), available: credits(e.data.available) }) : t("transfer.move.error.insufficientFee");
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? t("transfer.move.error.outOfRange", { min: credits(e.data.min), max: credits(e.data.max) }) : t("transfer.move.error.outOfRangeAmount");
    case "credits.move_pending": {
      const id = e.data?.kind === "artifact" ? e.data.artifact : null;
      const held = activity?.find((a) => a.id === id);
      return held?.kind === "send" && !held.cancels ? MOVE_COPY.waiting.heldSend : MOVE_COPY.waiting.held;
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
      return isAddressSendsOff(e) ? MAINNET_OFF_COPY.refused : t("transfer.move.error.featureOff");
    case "credits.rules_unsupported":
    case "credits.preflight_refused":
      return preflightText(e.code, stillHeld(activity, exitOf) ? earlierKind(moveExit(activity, exitOf)) : null);
    case "invalid_argument":
      return invalidArgumentText(e);
    default:
      return t("transfer.move.error.stopped", { code: e.code });
  }
}

/** Which pre-check text names the held item a refused exit leaves standing: a send's, a cancel's, a move's
 * or a contact payment's, else a top-up's. */
const earlierKind = (exit: MoveExit | null): "send" | "topUp" | "held" => (exit === null ? "topUp" : exit.kind === "send" ? "send" : "held");

/** Back is offered when a new quote may pass. */
export const moveCanRetry = (e: unknown) => hasCode(e, "platform.grant_exceeded") || hasCode(e, "platform.insufficient_credits") || hasCode(e, "credits.quote_changed");

/** A move, send, contact payment or top-up from Platform that Platform has not answered (`pending`) or
 * that may still execute (`mayLand`: also a `failed` one by height). They share one hold on the wallet's
 * Platform addresses. */
const holdingKind = (a: PlatformActivity) => a.kind === "moveToCore" || a.kind === "send" || a.kind === "contactPayment" || a.kind === "toIdentity";
export const isHeldOp = (a: PlatformActivity): boolean => holdingKind(a) && (a.state === "pending" || a.mayLand);

export const isPendingPlatformCredit = (a: PlatformActivity): boolean =>
  a.state === "pending" && (a.kind === "fromCore" || a.kind === "fromShielded" || a.kind === "recovered" || a.kind === "registrationRefund" || a.kind === "incoming");

/** Whether a move or send of this wallet holds the Platform addresses: recorded `pending` or able to
 * execute still (it settles within minutes: the engine's `platform_activity` settles it). While one
 * does, the engine refuses any other move or send that is not its exact retry. */
export const hasPendingMove = (activity: readonly PlatformActivity[] | null | undefined): boolean => !!activity?.some(isHeldOp);

/** Whether a move or send is recorded `pending`: Platform has not answered, so not even its retry
 * is taken yet. */
export const hasUnansweredOp = (activity: readonly PlatformActivity[] | null | undefined): boolean =>
  !!activity?.some((a) => holdingKind(a) && a.state === "pending");

/** The moves, sends, contact payments and top-ups from Platform that hold the addresses and Platform has
 * answered, the newest first: on mainnet, where new moves are off, Move stays offered as their exit (an
 * unanswered one holds every move until it settles). A cancel is not one: its move or payment's
 * document settles it. */
export const heldExitOps = (activity: readonly PlatformActivity[] | null | undefined): PlatformActivity[] => activity?.filter((a) => !a.cancels && a.state !== "pending" && isHeldOp(a)) ?? [];

/** The held send `a` is, which "Move to Dash wallet" is the way out of; `null` for any other row. */
export const isHeldSend = (a: PlatformActivity): boolean => a.kind === "send" && !a.cancels && a.state === "failed" && a.mayLand && a.to !== null;

/** A held cancel: "Move to Dash wallet" is its way out beside its own exact re-send. */
export const isHeldCancel = (a: PlatformActivity): boolean => a.kind === "send" && !!a.cancels && a.state === "failed" && a.mayLand;

/** What the cancel of `a` would be: of a held move to the Dash wallet or contact payment, and only
 * while no send, top-up or other cancel holds beside it (its document could not tell those apart). Its
 * own cancel that Platform answered and that still holds is offered again: the engine re-sends it as signed.
 * While `sendsOff` (mainnet, where this version keeps new moves off) only that re-send is offered: a new
 * cancel is not, and the move or payment waits for Platform. */
export function cancelOf(a: PlatformActivity, activity: readonly PlatformActivity[] | null | undefined, sendsOff = false): "move" | "payment" | null {
  if (a.state !== "failed" || !a.mayLand || (a.kind !== "moveToCore" && a.kind !== "contactPayment")) return null;
  const blocks = (b: PlatformActivity) => (b.cancels ? b.cancels !== a.id : b.kind !== "moveToCore" && b.kind !== "contactPayment");
  const beside = activity?.some((b) => b.id !== a.id && isHeldOp(b) && (b.state === "pending" || blocks(b)));
  if (beside || (sendsOff && !activity?.some((b) => b.cancels === a.id && isHeldOp(b)))) return null;
  return a.kind === "moveToCore" ? "move" : "payment";
}

/** The newest of `heldExitOps`, or `null`. */
export const heldExitOp = (activity: readonly PlatformActivity[] | null | undefined): PlatformActivity | null => heldExitOps(activity)[0] ?? null;

/** Whether a payment (a `send`, not a move) holds the Platform addresses and no move does: the Move card says so. */
export const sendHolds = (activity: readonly PlatformActivity[] | null | undefined): boolean =>
  !!activity?.some((a) => a.kind === "send" && !a.cancels && isHeldOp(a)) && !activity.some((a) => a.kind === "moveToCore" && isHeldOp(a));

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
  /** The held top-up or send the last move was the exit of, or `null`. */
  exitOf: string | null = null;
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

  /** Confirm & move: the quote's grant, then `platformMoveToCore` to the quote's `to`, one of the
   * wallet's own receiving addresses (one call at a time). Everything, or as the exit of the quote's
   * `exitOf` exactly its planned withdrawal, so the engine moves what Confirm showed or refuses. */
  async move({ grant, to, exitOf, credits, feeCredits }: Pick<MoveToCoreQuote, "grant" | "to" | "exitOf" | "credits" | "feeCredits">): Promise<void> {
    const amount: WithdrawAmount = exitOf === null ? { kind: "all" } : { kind: "credits", credits: plannedWithdrawalCredits({ credits, feeCredits }) };
    if (this.running) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.exitOf = exitOf;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.platformMoveToCore(this.walletId, amount, to, exitOf, g.id);
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
