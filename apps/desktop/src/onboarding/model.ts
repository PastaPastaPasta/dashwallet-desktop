// The base of the U-02 view models: plain classes (no React) that announce changes, read by the
// screens through `useModel`. Secrets stay in private fields; nothing here persists.

import { useSyncExternalStore } from "react";
import { hasCode, isAppError, retryAfterSecs } from "../api/backend";
import type { ErrorCode } from "../api/generated/ErrorCode";

export abstract class Model {
  #version = 0;
  readonly #listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.#listeners.add(listener);
    return () => void this.#listeners.delete(listener);
  };

  version = () => this.#version;

  protected changed() {
    this.#version++;
    for (const l of this.#listeners) l();
  }
}

/** Re-renders when `model` changes; read its fields directly. */
export function useModel<M extends Model>(model: M): M {
  useSyncExternalStore(model.subscribe, model.version);
  return model;
}

/** A wallet the user picks in a flow, by name. */
export interface WalletChoice {
  walletId: string;
  name: string;
}

/** dash-qt `AskPassphraseDialog`. */
export const WRONG_PASSPHRASE = "The passphrase entered for the wallet decryption was incorrect.";
export const PASSPHRASE_REQUIRED = "Enter your wallet passphrase.";
export const INVALID_PHRASE = "The recovery phrase is not valid. Check every word.";

/** Copy for a failure chosen by its code alone (U-02 D16): the engine's `message` can quote the
 * input (a mnemonic error names the word), so it is never shown. */
const BY_CODE: Partial<Record<ErrorCode, string>> = {
  invalid_mnemonic: INVALID_PHRASE,
  wallet_already_exists: "This wallet has already been added.",
  "vault.wallet_secret_exists": "This wallet's keys are already in the wallet.",
  "vault.locked": "Unlock the wallet and try again.",
  "vault.mixing_only": "Unlock the wallet fully and try again.",
  "vault.passphrase_rejected": "The wallet rejected this passphrase. Choose another one.",
  "vault.os_store_unavailable": "This computer has no secret store to keep an unencrypted wallet's key. Choose a passphrase instead.",
  "vault.recovery_mismatch": "This recovery phrase does not belong to the selected wallet.",
  "vault.wrong_passphrase": WRONG_PASSPHRASE,
  "vault.no_secret": "This wallet has no recovery phrase.",
  "vault.no_vault": "This network has no wallet yet.",
  "vault.already_exists": "This network already has a wallet vault.",
  network_not_open: "The wallet is not open.",
};

export function failureText(e: unknown): string {
  if (!isAppError(e)) return "Something went wrong.";
  if (e.code === "vault.throttled") return tryAgainIn(retryAfterSecs(e));
  return BY_CODE[e.code] ?? `Something went wrong (${e.code}).`;
}

/** "6 minutes", "4 hours": minutes below two hours (rounded up), hours above. */
export function waitText(secs: number): string {
  const minutes = Math.max(1, Math.ceil(secs / 60));
  if (minutes < 120) return minutes === 1 ? "1 minute" : `${minutes} minutes`;
  const hours = Math.ceil(secs / 3600);
  return hours === 1 ? "1 hour" : `${hours} hours`;
}

/** iOS lockout copy. */
export const tryAgainIn = (secs: number) => `Wallet disabled, please try again in ${waitText(secs)}`;

/** Why a passphrase was not accepted: wrong (with the vault's wait, when it set one), or the
 * wait itself, or another failure. */
export function unlockFailureText(e: unknown): string {
  const wait = retryAfterSecs(e);
  if (hasCode(e, "vault.wrong_passphrase")) return wait > 0 ? `${WRONG_PASSPHRASE} ${tryAgainIn(wait)}` : WRONG_PASSPHRASE;
  return failureText(e);
}
