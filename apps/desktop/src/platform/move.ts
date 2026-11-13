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
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";

export type PlatformMoveApi = Pick<Backend, "platformMoveToCore" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const MOVE_COPY = {
  card: {
    title: "Platform addresses",
    /** What one move cannot take (below the minimum input, beyond the inputs one transition takes, or the fee reserve): it stays on Platform. */
    dust: (amount: string) => `${amount} on your Platform addresses cannot be moved to your Dash wallet right now.`,
  },
  message: "Everything on your Platform addresses moves to your Dash wallet. Dash Platform pays it to the address below.",
  rows: { taken: "Taken from Platform, about", fee: "Network fee, about", receive: "You receive about", to: "To your Dash wallet" },
  quoting: "Working out the fee…",
  quoteFailed: "The fee could not be worked out.",
  back: "Back",
  go: "Confirm & move",
  cancel: "Cancel",
  running: { title: "Moving…", text: "Dash Platform confirms the move. You can close this: Activity shows it." },
  done: {
    title: "Moved",
    text: (amount: string, fee: string | null) =>
      `About ${amount} is on its way to your Dash wallet.${fee === null ? "" : ` The network fee was ${fee}.`} It shows in Activity once it arrives.`,
    /** The engine proved the debit but not what arrives. */
    debitOnly: (taken: string) => `${taken} left your Platform addresses. Activity shows what arrives in your Dash wallet.`,
  },
  pending: {
    title: "Not confirmed yet",
    text: (amount: string) => `Dash Platform has not said whether the move of about ${amount} went through. Activity shows it when it does. Do not move again until it does.`,
    debitOnly: (taken: string) => `Dash Platform has not said whether the move of ${taken} went through. Activity shows it when it does. Do not move again until it does.`,
  },
  /** A move the engine superseded with a retry: not a loss, and nothing to wait for. */
  replaced: { title: "Replaced by a retry", text: "This move was replaced by a retry. Activity shows the retry." },
  unseen: "Your move has an update.",
  waiting: {
    text: "A move to your Dash wallet is not confirmed yet. Do not move again until Activity shows that it went through.",
    held: "An earlier payment or move from Platform may still go through. Wait for it to finish, then move again.",
    /** A payment (not a move) from Platform is unsettled: Move waits until it settles. */
    sendHeld: "A payment from Platform may still go through. Moving waits until it settles.",
    /** Move refused beside a held payment: that payment's own retry (in Pay) is the way out. */
    heldSend: "An earlier payment from Platform may still go through. Wait for it to finish, or send that same payment again from Pay and press Retry that payment, then move again.",
    action: "View in Activity",
  },
  /** Before Move is offered, Activity must say no move is unfinished (after a relaunch the flow remembers nothing). */
  check: { loading: "Checking for an unfinished move…", failed: "Could not check for an unfinished move.", again: "Check again" },
  show: "Show",
  close: "Close",
} as const;

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
  return detail.trim() === "" ? "That was not accepted." : /[.!?]$/.test(detail) ? detail : `${detail}.`;
}

/** A move's refusal, by code. `credits` shows a credit figure as Dash. */
export function moveErrorText(e: unknown, credits: (c: string) => string, activity?: readonly PlatformActivity[] | null): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? `Not enough on your Platform addresses: ${credits(e.data.needed)} needed with the fee, ${credits(e.data.available)} available.` : "There is not enough on your Platform addresses to cover the fee.";
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? `Dash Platform moves between ${credits(e.data.min)} and ${credits(e.data.max)} at a time.` : "Dash Platform does not take a move of this amount.";
    case "credits.move_pending": {
      const id = e.data?.kind === "artifact" ? e.data.artifact : null;
      return activity?.find((a) => a.id === id)?.kind === "send" ? MOVE_COPY.waiting.heldSend : MOVE_COPY.waiting.held;
    }
    case "platform.broadcast_unknown":
      return "It may have been sent. Check Activity before trying again.";
    case "platform.context_unavailable":
      return "Waiting for the network to sync and verify your Platform addresses. Try again shortly.";
    case "platform.trust_mismatch":
      return "Your Platform addresses could not be verified against the network. Nothing was moved.";
    case "platform.unavailable":
    case "platform.timeout":
      return "Dash Platform did not answer. Try again.";
    case "platform.signer_unavailable":
      return "Unlock the wallet fully and try again.";
    case "platform.grant_exceeded":
      return "The cost went above what you approved. Try again.";
    case "platform.feature_off":
      return "Platform addresses are turned off.";
    case "invalid_argument":
      return invalidArgumentText(e);
    default:
      return `It stopped with ${e.code}.`;
  }
}

/** Back is offered when a new quote may pass. */
export const moveCanRetry = (e: unknown) => hasCode(e, "platform.grant_exceeded") || hasCode(e, "platform.insufficient_credits");

/** A move or send Platform has not answered (`pending`) or that may still execute (`mayLand`: also a
 * `failed` one by height). Moves and sends share one hold on the wallet's Platform addresses. */
export const isHeldOp = (a: PlatformActivity): boolean => (a.kind === "moveToCore" || a.kind === "send") && (a.state === "pending" || a.mayLand);

/** Whether a move or send of this wallet holds the Platform addresses: recorded `pending` or able to
 * execute still (it settles within minutes: the engine's `platform_activity` settles it). While one
 * does, the engine refuses any other move or send that is not its exact retry. */
export const hasPendingMove = (activity: readonly PlatformActivity[] | null | undefined): boolean => !!activity?.some(isHeldOp);

/** Whether a move or send is recorded `pending`: Platform has not answered, so not even its retry
 * is taken yet. */
export const hasUnansweredOp = (activity: readonly PlatformActivity[] | null | undefined): boolean =>
  !!activity?.some((a) => (a.kind === "moveToCore" || a.kind === "send") && a.state === "pending");

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
   * of the wallet's own receiving addresses (one call at a time). */
  async move(grant: GrantRequest, to: string): Promise<void> {
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
        return this.api.platformMoveToCore(this.walletId, { kind: "all" }, to, g.id);
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
