// The In progress group (MG-10 §1.10), the operations' share of the sidebar badge, and the Auto
// sidebar rule (§3.6). Shared sessions join in S5: until then Auto and the badge read the list,
// its coins and the ProTx operations only.

import type { AppError } from "../api/generated/commands";
import type { MasternodeOverview } from "../api/generated/MasternodeOverview";
import type { ProTxOperation } from "../api/generated/ProTxOperation";
import type { ProTxOperationKindName } from "../api/generated/ProTxOperationKindName";
import { sendErrorText } from "../screens/pay/model";
import { t } from "../i18n";
import { needsAttention } from "./model";

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
      return { ...base, status: error ? t(`${P}.status.notSent`, { reason: error }) : t(`${P}.status.notSentPlain`), primary: "resend", more: ["discard"] };
    case "sent":
      return {
        title,
        status: t(`${P}.status.sent`, { title }),
        overdueLine: o.overdue ? t(hasFeeInputs(o) ? `${P}.overdue` : `${P}.overdueDissolve`) : null,
        activityTxid: o.overdue && hasFeeInputs(o) ? o.txid : null,
        primary: "resend",
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

/** S2's operations in the badge (§3.6): work only the user can move on. */
export const opsBadge = (ops: readonly ProTxOperation[] | undefined) => (ops ?? []).filter((o) => ["needs_signature", "inputs_spent", "not_sent"].includes(o.state.kind)).length;

/** The badge's masternode half: banned own nodes (S1) and the operations (S2). */
export const masternodesBadge = (o: MasternodeOverview | undefined, ops: readonly ProTxOperation[] | undefined) => (o?.masternodes.filter(needsAttention).length ?? 0) + opsBadge(ops);

/** Auto shows the item for a mine or tracked row, a collateral-shaped coin, or unsettled operations. */
export function autoShows(o: MasternodeOverview | undefined, ops: readonly ProTxOperation[] | undefined): boolean {
  if (o && (o.masternodes.some((r) => r.mine || r.tracked) || o.collateralCoins.length > 0)) return true;
  return inProgress(ops ?? []).length > 0;
}
