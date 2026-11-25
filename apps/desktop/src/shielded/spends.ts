// Where this build's engine cannot spend from Shielded (`ShieldedStatus.spendsAvailable`): Send from
// Shielded, the moves to the Dash wallet and to Platform, paying a contact from Shielded, and every
// way into Shielded (shielding, CoinJoin's move, Receive's address) wait with one reason line, and
// the engine's refusal of a spend or a new shield reads the same. Finishing a shield already sent, sync, the
// balance and history carry on. The engine's flag is the only switch.

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
};
