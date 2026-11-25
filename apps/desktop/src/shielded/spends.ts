// Where this build's engine cannot spend from Shielded (`ShieldedStatus.spendsAvailable`): Send from
// Shielded, the moves to the Dash wallet and to Platform, and paying a contact from Shielded wait
// with one reason line; every way into Shielded (shielding from the Dash wallet, CoinJoin or Platform)
// waits with another, and Receive shows no Shielded address. The engine refuses the same calls. Finishing
// a shield already sent, sync, the balance and history carry on. The engine's flag is the only switch.

import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { t } from "../i18n";
import type { ShieldedKind } from "./flow";

/** Shielded is on and this build cannot spend from it. `null` (no command, or not read yet) is not off. */
export function shieldedSpendsOff(status: Pick<ShieldedStatus, "active" | "spendsAvailable"> | null | undefined): boolean {
  return !!status && status.active && !status.spendsAvailable;
}

const SPENDS: ReadonlySet<ShieldedKind | undefined> = new Set<ShieldedKind | undefined>(["send", "out", "toPlatform", "add"]);

/** Whether the refusal `code` of a `kind` call is the engine refusing a spend, or a new shield, this build cannot make. */
export const spendsOffRefusal = (code: string, kind: ShieldedKind | undefined, spendsOff: boolean): boolean => spendsOff && code === "platform.unavailable" && SPENDS.has(kind);

export const SHIELDED_SPENDS_COPY = {
  /** Where a spend from Shielded would be. */
  get unavailable() {
    return t("transfer.shielded.spends.unavailable");
  },
  /** A source picker's Shielded row. */
  get source() {
    return t("transfer.shielded.spends.source");
  },
  /** Where a move into Shielded would be. */
  get shieldIn() {
    return t("transfer.shielded.shieldInUnavailable");
  },
  /** Receive's Shielded tab, in place of the address. */
  get receive() {
    return t("transfer.shielded.receiveUnavailable");
  },
};

/** The reason a `kind` held by this build gives: a move into Shielded (`add`) or a spend from it. */
export const spendsOffText = (kind: ShieldedKind | undefined): string => (kind === "add" ? SHIELDED_SPENDS_COPY.shieldIn : SHIELDED_SPENDS_COPY.unavailable);
