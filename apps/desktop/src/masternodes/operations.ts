// The In progress group (MG-10 §1.10). Shared sessions join it in S5.

import type { AppError } from "../api/generated/commands";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxOperationKindName } from "../api/generated/ProTxOperationKindName";
import { sendErrorText } from "../screens/pay/model";
import { t } from "../i18n";
import { REGISTRATION_SENDS } from "./registration";

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

/** Send again, except on a registration while registrations don't send (D14). */
export const resends = (o: ProTxOperation, sends: boolean = REGISTRATION_SENDS) => o.kind !== "registration" || sends;

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

/** One row of §1.10's table, by state. */
export function opRow(o: ProTxOperation): OpRowView {
  const title = opTitle(o.kind);
  const error = lastErrorText(o.lastError);
  const base = { title, overdueLine: null, activityTxid: null };
  switch (o.state.kind) {
    case "needs_signature":
      return { ...base, status: t(`${P}.status.needsSignature`), primary: "continue", more: ["discard"] };
    case "inputs_spent":
      return { ...base, status: t(`${P}.status.inputsSpent`), primary: "refresh", more: ["discard"] };
    case "signed":
    case "not_sent":
      return { ...base, status: error ? t(`${P}.status.notSent`, { reason: error }) : t(`${P}.status.notSentPlain`), primary: resends(o) ? "resend" : null, more: ["discard"] };
    case "sent":
      return {
        title,
        status: t(`${P}.status.sent`, { title }),
        overdueLine: o.overdue ? t(hasFeeInputs(o) ? `${P}.overdue` : `${P}.overdueDissolve`) : null,
        activityTxid: o.overdue && hasFeeInputs(o) ? o.txid : null,
        primary: resends(o) ? "resend" : null,
        more: [],
      };
    default:
      return { ...base, status: "", primary: null, more: [] };
  }
}

/** The resend confirm: `sent` sends the same bytes again; both cost at most the row's `debit` (EP 2). */
export const resendConfirmText = (o: ProTxOperation, amount: string) => t(o.state.kind === "sent" ? `${P}.resendSent` : `${P}.resendNotSent`, { debit: amount });

/** The grant cap of Send again: the engine's `debit`, never the fee (§3.4). */
export const resendCap = (o: ProTxOperation) => o.debit;
