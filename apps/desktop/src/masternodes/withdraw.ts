// The evonode credit withdrawal's destination row and its unverified refusals (MG-13, H-09), for
// the withdraw sheet MG-10 S4 builds on `mn_withdrawal_quote`.

import { hasCode } from "../api/backend";
import { t } from "../i18n";

/** Why a destination still reads `checking` (MG-13 `WithdrawalCheckingReason`). */
export type WithdrawCheckingReason = "platform_behind" | "payout_change_pending" | "unverified" | "spv_unsynced";

/** Where a withdrawal by one key pays (MG-13 `WithdrawalDestinationCheck`), as the quote's
 * `owner_destination` and `payout_destination` carry it. */
export type WithdrawDestination = { kind: "fixed"; address: string } | { kind: "checking"; reason: WithdrawCheckingReason };

/** The destination row: the address once fixed, "Checking…" while it is not. */
export function withdrawDestinationText(d: WithdrawDestination): string {
  return d.kind === "fixed" ? d.address : t("masternodes.withdraw.checking");
}

const unverifiedText = () => t("masternodes.withdraw.unverified");

/** The withdrawal's refusals over unverified identity data; `null` for any other. `destination`:
 * the quote's destination for the chosen key, which says why `withdrawal_destination_checking`
 * still checks (its error carries no reason). */
export function withdrawUnverifiedText(e: unknown, destination?: WithdrawDestination | null): string | null {
  if (hasCode(e, "masternodes.withdrawal_unverified")) return unverifiedText();
  if (hasCode(e, "masternodes.withdrawal_destination_checking") && destination?.kind === "checking" && destination.reason === "unverified") return unverifiedText();
  return null;
}

/** What the sheet says under a destination that still checks; `null` for one that is fixed or
 * checks for a reason other than unverified data. */
export function destinationNote(d: WithdrawDestination | null | undefined): string | null {
  return d?.kind === "checking" && d.reason === "unverified" ? unverifiedText() : null;
}
