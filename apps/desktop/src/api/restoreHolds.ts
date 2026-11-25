// The R3 restore holds' DTO fields (Q92 H1, H2 and the RESTORED money gate's refusal). The engine adds
// them to Utxo, TxDetailExtrasInfo and TxActionRefusalName; RESTORED itself is storeOpen.ts's. Until the generated bindings carry them, these read them
// from the answer as it arrives, and an answer without them reads as no hold.

import type { TxActionRefusalName } from "./generated/TxActionRefusalName";
import type { TxDetailExtrasInfo } from "./generated/TxDetailExtrasInfo";
import type { Utxo } from "./generated/Utxo";

/** H1: a coin a restored, still unmined payment of this wallet sent. Shown, never offered, and
 * counted in `unconfirmed`, never as spendable. */
export type HeldUtxo = Utxo & { pendingUnproven?: boolean };

/** H2: the payment still holds `restore_hold` from before the restore. */
export type HeldTxExtras = TxDetailExtrasInfo & { restoreHeld?: boolean };

/** The engine's refusal of a transaction action while RESTORED is set. */
export type HeldTxActionRefusalName = TxActionRefusalName | "restored";

export const isRestoredRefusal = (refusal: TxActionRefusalName): boolean => (refusal as HeldTxActionRefusalName) === "restored";

export const pendingUnproven = (u: Utxo): boolean => (u as HeldUtxo).pendingUnproven === true;

export const restoreHeld = (x: TxDetailExtrasInfo): boolean => (x as HeldTxExtras).restoreHeld === true;

/** What RESTORED still waits for (`restore_status`, B addendum 1). */
export type RestoreWait = "coreSync" | "platformPass" | "historyLoad";

/** `restore_status`: `null` while RESTORED isn't set. `historyWallets`: the wallets whose history
 * failed to load; `clearFailed`: the check finished but its clear couldn't be written. */
export type RestoreStatus = { waiting: RestoreWait[]; historyWallets: string[]; clearFailed: boolean };

