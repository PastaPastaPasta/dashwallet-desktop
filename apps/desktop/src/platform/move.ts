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
import type { MoveToCoreOutcome } from "../api/pending";
import { platformOp } from "../dashpay/topup";
import { failureText, Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";

export type PlatformMoveApi = Pick<Backend, "platformMoveToCore" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export const MOVE_COPY = {
  action: "Move to Dash wallet",
  card: {
    title: "Platform addresses",
    text: "Your Platform balance is not in your Dash wallet yet. Move it there to spend it as Dash.",
  },
  title: "Move to Dash wallet",
  message: "Everything on your Platform addresses moves to your Dash wallet. Dash Platform pays it to the address below.",
  rows: { taken: "Taken from Platform", fee: "Network fee", receive: "You receive about", to: "To your Dash wallet" },
  quoting: "Working out the fee…",
  back: "Back",
  go: "Confirm & move",
  cancel: "Cancel",
  running: { title: "Moving…", text: "Dash Platform confirms the move. You can close this: Activity shows it." },
  done: {
    title: "Moved",
    text: (amount: string) => `About ${amount} is on its way to your Dash wallet. It shows in Activity once it arrives.`,
  },
  pending: {
    title: "Not confirmed yet",
    text: (amount: string) => `Dash Platform has not said whether the move of about ${amount} went through. Activity shows it when it does. Do not move again until it does.`,
  },
  unseen: "Your move has an update.",
  show: "Show",
  close: "Close",
} as const;

export type MoveStep = "confirm" | "running" | "done" | "refused";

/** A move's refusal, by code. `credits` shows a credit figure as Dash. */
export function moveErrorText(e: unknown, credits: (c: string) => string): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "platform.insufficient_credits":
      return e.data?.kind === "credits" ? `Not enough on your Platform addresses: ${credits(e.data.needed)} needed with the fee, ${credits(e.data.available)} available.` : "There is not enough on your Platform addresses to cover the fee.";
    case "credits.withdrawal_out_of_range":
      return e.data?.kind === "creditRange" ? `Dash Platform moves between ${credits(e.data.min)} and ${credits(e.data.max)} at a time.` : "Dash Platform does not take a move of this amount.";
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
    default:
      return `It stopped with ${e.code}.`;
  }
}

/** Back is offered when a new quote may pass. */
export const moveCanRetry = (e: unknown) => hasCode(e, "platform.grant_exceeded") || hasCode(e, "platform.insufficient_credits");

export class PlatformMoveFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: MoveStep = "confirm";
  outcome: MoveToCoreOutcome | null = null;
  error: unknown = null;
  running = false;
  /** Called after a call that ran has ended: what the move changed is read again. */
  onSettled: (() => void) | null = null;
  readonly grants: GrantModel;

  constructor(
    private readonly api: PlatformMoveApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** A result that came while the sheet was hidden, not yet seen. */
  get unseen(): boolean {
    return !this.open && !this.running && (this.step === "done" || this.step === "refused");
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

  /** Hides the sheet; a running call goes on and its result waits. A result seen is dropped. */
  close() {
    this.open = false;
    if (!this.running) {
      if (this.step === "done" || this.step === "refused") this.step = "confirm";
      this.grants.cancel();
    }
    this.changed();
  }

  /** Confirm & move: the quote's grant, then `platformMoveToCore` for everything (one call at a time). */
  async move(grant: GrantRequest): Promise<void> {
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
        return this.api.platformMoveToCore(this.walletId, { kind: "all" }, g.id);
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
      if (ran) queueMicrotask(() => this.onSettled?.());
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
