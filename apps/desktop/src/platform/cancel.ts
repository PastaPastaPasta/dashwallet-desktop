// The cancel of a held move to the Dash wallet or contact payment from Platform: the same inputs, at
// the same nonces and amounts, go to one of the wallet's own Platform addresses instead. Only one of
// the two goes through, and the held item's withdrawal document says which. The quote
// (`platform_cancel_quote`) names the amount, the fee and the address; Confirm runs `platform_cancel`
// under the quote's `platformOp` grant. One flow per wallet, kept while the app runs, so the details
// sheet can close while the call runs and reopen to its result.

import type { Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformCancelQuote } from "../api/generated/PlatformCancelQuote";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import { platformOp } from "../dashpay/topup";
import { t } from "../i18n";
import { Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { exitLinkOf, MOVE_COPY, onMoveSettledNotify } from "./move";

export type PlatformCancelApi = Pick<Backend, "platformCancelQuote" | "platformCancel" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly">;

export type CancelKind = "move" | "payment";

export { recipientOf } from "./move";

export const CANCEL_COPY = {
  action: (kind: CancelKind) => t(`platform.cancel.${kind}.action`),
  title: (kind: CancelKind) => t(`platform.cancel.${kind}.title`),
  body: (kind: CancelKind, amount: string, fee: string, recipient: string) => t(`platform.cancel.${kind}.body`, { amount, fee, recipient }),
  confirm: (kind: CancelKind) => t(`platform.cancel.${kind}.confirm`),
  done: (kind: CancelKind, recipient: string) => t(`platform.cancel.${kind}.done`, { recipient }),
  lost: (kind: CancelKind, recipient: string) => t(`platform.cancel.${kind}.lost`, { recipient }),
  /** "Move to Dash wallet" went through in place of the cancel and the held item. */
  exited: (kind: CancelKind, recipient: string) => (kind === "payment" ? MOVE_COPY.exitCancel.done.payment(recipient) : MOVE_COPY.exitCancel.done.move),
  /** The group settled, and Activity does not say which of its members went through. */
  unknown: (kind: CancelKind) => t(kind === "payment" ? "pay.status.settledUnknown.text" : "transfer.status.settledUnknown.text"),
  /** The held item is too small to cancel (`credits.send_out_of_range`): Move to Dash wallet instead. */
  tooSmall: (kind: CancelKind, amount: string, recipient: string) => t(`platform.cancel.${kind}.tooSmall`, { amount, recipient }),
  get dismiss() {
    return t("platform.cancel.dismiss");
  },
  get pending() {
    return t("platform.cancel.pending");
  },
};

export type CancelResult = "done" | "lost" | "exited" | "unknown" | "pending";

/** Where the cancel of the held item `of` stands, from Activity's rows of its group (`activity`), else
 * what the call returned: `done` once a cancel of `of` went through, `lost` once `of` did, `exited` once
 * "Move to Dash wallet" out of one of its cancels did, `pending` while any of them may still go through,
 * else `unknown`. A failed cancel alone never says which one went through. */
export function cancelResult(outcome: PlatformSendOutcome, activity: readonly PlatformActivity[] | null | undefined, of: string): CancelResult {
  const won = (a: Pick<PlatformActivity, "state" | "winnerKnown"> | undefined) => a?.state === "done" && a.winnerKnown !== false;
  const own = activity?.find((a) => a.id === outcome.id);
  if (!own && won(outcome)) return "done";
  if (!activity) return "pending";
  const cancels = activity.filter((a) => a.cancels === of);
  const ids = cancels.map((c) => c.id);
  const exits = activity.filter((a) => a.kind === "moveToCore" && ids.includes(exitLinkOf(a.id) ?? ""));
  const held = activity.find((a) => a.id === of);
  if (cancels.some(won)) return "done";
  if (won(held)) return "lost";
  if (exits.some(won)) return "exited";
  const holds = (a: Pick<PlatformActivity, "state" | "mayLand">) => a.state === "pending" || a.mayLand;
  return [...(held ? [held] : []), ...cancels, ...exits, ...(own ? [] : [outcome])].some(holds) ? "pending" : "unknown";
}

/** What the cancel's outcome says, by its `cancelResult`. `recipient`: the cancelled payment's. */
export const cancelResultText = (result: CancelResult, kind: CancelKind, recipient: string): string =>
  result === "pending" ? CANCEL_COPY.pending : result === "unknown" ? CANCEL_COPY.unknown(kind) : CANCEL_COPY[result](kind, recipient);

export type CancelStep = "confirm" | "running" | "done" | "refused";

export class PlatformCancelFlow extends Model {
  /** The held move or payment shown for cancelling, or `null`. */
  of: string | null = null;
  step: CancelStep = "confirm";
  quote: PlatformCancelQuote | null = null;
  quoteError: unknown = null;
  outcome: PlatformSendOutcome | null = null;
  error: unknown = null;
  running = false;
  readonly grants: GrantModel;

  constructor(
    private readonly api: PlatformCancelApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Shows the cancel of `of` and asks for its quote. A running cancel, or one of `of` with a result,
   * stays. */
  async begin(of: string): Promise<void> {
    if (this.running || (this.of === of && this.step !== "confirm")) return;
    this.of = of;
    this.step = "confirm";
    this.quote = null;
    this.quoteError = null;
    this.outcome = null;
    this.error = null;
    this.changed();
    try {
      const quote = await this.api.platformCancelQuote(this.walletId, of);
      if (this.of === of && this.step === "confirm") this.quote = quote;
    } catch (e) {
      if (this.of === of) this.quoteError = e;
    }
    this.changed();
  }

  /** Keep waiting: back to the details. A running call goes on; its result shows when `of` is shown again. */
  dismiss() {
    if (this.running) return;
    if (this.step !== "done") this.of = null;
    this.grants.cancel();
    this.changed();
  }

  /** Drops a seen result. */
  close() {
    if (this.running) return;
    this.of = null;
    this.step = "confirm";
    this.outcome = null;
    this.error = null;
    this.quote = null;
    this.changed();
  }

  /** Cancel: the quote's grant, then `platformCancel` to the quote's own address (one call at a time). */
  async confirm(): Promise<void> {
    const q = this.quote;
    if (this.running || q === null || this.of !== q.of) return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(q.grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.platformCancel(this.walletId, q.of, q.to, g.id);
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
      if (ran) onMoveSettledNotify();
      this.changed();
    }
  }
}

const flows = new Map<string, PlatformCancelFlow>();

/** The wallet's cancel flow, created on first use and kept while the app runs. */
export function cancelFor(api: PlatformCancelApi, walletId: string): PlatformCancelFlow {
  let flow = flows.get(walletId);
  if (!flow) {
    flow = new PlatformCancelFlow(api, walletId);
    flows.set(walletId, flow);
  }
  return flow;
}

/** Test hook: forgets every wallet's flow. */
export function resetPlatformCancels() {
  flows.clear();
}
