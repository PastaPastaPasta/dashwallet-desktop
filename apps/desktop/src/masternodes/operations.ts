// The In progress group (MG-10 §1.10). Shared sessions join it in S5.

import type { AppError } from "../api/generated/commands";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxOperationKindName } from "../api/generated/ProTxOperationKindName";
import { sendErrorText } from "../screens/pay/model";
import { t } from "../i18n";
import { registrarUpdateSends } from "./changeKeys";
import { registrationSends } from "./registration";

const P = "masternodes.inProgress";

/** Settled operations leave the group: `confirmed`, and `conflicted` (shown once as a notice). */
export const inProgress = (ops: readonly ProTxOperation[]) => ops.filter((o) => o.state.kind !== "confirmed" && o.state.kind !== "conflicted");

export const conflicted = (ops: readonly ProTxOperation[]) => ops.filter((o) => o.state.kind === "conflicted");

export const opTitle = (kind: ProTxOperationKindName) => t(`${P}.title.${kind}`);

/** `last_error` is a wire code (`send.no_peers`, …): Pay's sentence for it, else the code. */
export function lastErrorText(code: string | null): string | null {
  if (code === null) return null;
  const e: AppError = { code: code as AppError["code"], message: code, data: null };
  return sendErrorText(e, "test")?.text ?? t(`${P}.unknownError`, { code });
}

/** A shared dissolution's send result: the warning when the network didn't take it, `null` once sent. */
export function handoffRefusedText(o: ProTxOperation): string | null {
  if (o.state.kind !== "not_sent" && o.state.kind !== "signed") return null;
  const reason = lastErrorText(o.lastError)?.replace(/\.$/, "");
  return reason ? t("masternodes.shared.dissolve.notSent", { reason }) : t("masternodes.shared.dissolve.notSentPlain");
}

/** Which held kinds may be acted on. Unknown network (still loading) stays off. */
export const proTxSends = (network: { kind: string } | null | undefined): Partial<Record<ProTxOperationKindName, boolean>> => ({
  registration: registrationSends(network),
  update_registrar: registrarUpdateSends(network),
});

/** A registration or registrar update row is acted on only while its kind sends (D14); Discard always stays.
 * Callers pass `proTxSends(network)`. */
export const actsOn = (o: ProTxOperation, sends: Partial<Record<ProTxOperationKindName, boolean>> = proTxSends(null)) =>
  sends[o.kind] ?? true;

/** A fee-input row can be abandoned in Activity; a dissolve row locks no coin (MG-04 D49, mn §2.4). */
const hasFeeInputs = (o: ProTxOperation) => o.kind !== "dissolve";

export type OpAction = "continue" | "refresh" | "resend" | "discard";

export interface OpRowView {
  title: string;
  status: string;
  /** The D49 line once `sent` is overdue, or the dissolve row's no-lock line. */
  overdueLine: string | null;
  /** Open in Activity, filtered to the txid (overdue fee-input rows). */
  activityTxid: string | null;
  primary: OpAction | null;
  more: OpAction[];
}

const WAITS_FOR_BLOCK: ReadonlySet<ProTxOperationKindName> = new Set(["registration", "update_registrar"]);

/** One row of §1.10's table, by state. `sends` defaults to an unknown network: registrations and registrar updates stay off. */
export function opRow(o: ProTxOperation, sends: Partial<Record<ProTxOperationKindName, boolean>> = proTxSends(null)): OpRowView {
  const title = opTitle(o.kind);
  const error = lastErrorText(o.lastError);
  const base = { title, overdueLine: null, activityTxid: null };
  const on = actsOn(o, sends);
  switch (o.state.kind) {
    case "needs_signature":
      return { ...base, status: t(`${P}.status.needsSignature`), primary: on ? "continue" : null, more: ["discard"] };
    case "inputs_spent":
      return { ...base, status: t(`${P}.status.inputsSpent`), primary: on ? "refresh" : null, more: ["discard"] };
    case "signed":
    case "not_sent":
      return { ...base, status: error ? t(`${P}.status.notSent`, { reason: error }) : t(`${P}.status.notSentPlain`), primary: on ? "resend" : null, more: ["discard"] };
    case "sent":
      return {
        title,
        status: t(`${P}.status.sent`, { title }),
        overdueLine: o.overdue ? t(hasFeeInputs(o) ? `${P}.overdue` : `${P}.overdueDissolve`) : null,
        activityTxid: o.overdue && hasFeeInputs(o) ? o.txid : null,
        // A registration or registrar update just sent waits for its block: Send again only once overdue.
        primary: on && (o.overdue || !WAITS_FOR_BLOCK.has(o.kind)) ? "resend" : null,
        more: [],
      };
    case "abandoned":
      // Abandoned in Activity (D49): its coins are back, it is never resent, and it may still be mined.
      return { title, status: t(`${P}.status.abandoned`, { title }), overdueLine: t(`${P}.abandoned`), activityTxid: o.txid, primary: null, more: [] };
    default:
      return { ...base, status: "", primary: null, more: [] };
  }
}

/** The resend confirm: `sent` sends the same bytes again; both cost at most the row's `debit` (EP 2). */
export const resendConfirmText = (o: ProTxOperation, amount: string) => t(o.state.kind === "sent" ? `${P}.resendSent` : `${P}.resendNotSent`, { debit: amount });

/** The grant cap of Send again: the engine's `debit`, never the fee (§3.4). */
export const resendCap = (o: ProTxOperation) => o.debit;
