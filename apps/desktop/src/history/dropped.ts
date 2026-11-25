// A transaction Dash Wallet dropped itself because it can never confirm: the line its details show,
// from the cause and the state the engine gives (never worked out here), and who a dropped payment
// was to.

import type { TxCounterparty } from "../api/generated/TxCounterparty";
import type { TxDropInfo } from "../api/generated/TxDropInfo";
import { contactName } from "../format/tx";
import { t } from "../i18n";

/** The details' line for a dropped transaction: where the engine says the coins it used are. */
export function droppedLine(d: TxDropInfo): string {
  if (d.cause === "invalidClaim") return t("activity.status.droppedClaim");
  const tools = t("routes.tools");
  switch (d.state.kind) {
    case "released":
      return t("activity.status.dropped");
    case "releasing":
      return t("activity.status.droppedReleasing");
    case "held":
      return t("activity.status.droppedHeld", { whatCanIDo: t("activity.status.whatCanIDo") });
    case "rescanNeeded":
      return t("activity.status.droppedRescan", { tools, rescan: t("tools.repair.rescanButton") });
    case "rescanFromHeight":
      return t("activity.status.droppedRescanFrom", { tools, birthTitle: t("tools.repair.birthTitle"), height: String(d.state.height) });
    case "awaitingFunding":
      return t("activity.status.droppedAwaitingFunding");
  }
}

/** Whether the details say what happened to a payment built on a transaction that was never valid
 * (`activity.invalidSource.detail`): only once its coins are back, which that text says. */
export function invalidSourceDetail(d: TxDropInfo): boolean {
  return d.cause === "invalidSource" && d.state.kind === "released";
}

/** Who a dropped payment was to: the contact's name, else the address's label, else the address. */
export function recipientOf(row: { counterparty?: TxCounterparty | null; label: string | null; address: string | null }): string | null {
  return contactName(row.counterparty ?? undefined) ?? row.label ?? row.address;
}
