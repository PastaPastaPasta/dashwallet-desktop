// The base of the U-02 view models: plain classes (no React) that announce changes, read by the
// screens through `useModel`. Secrets stay in private fields; nothing here persists.

import { useSyncExternalStore } from "react";
import { hasCode, isAppError, retryAfterSecs } from "../api/backend";
import type { ErrorCode } from "../api/generated/ErrorCode";
import { messageTable, t } from "../i18n";
import { flatten, type KeyOf } from "../i18n/core";
import { en } from "../i18n/en";

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

/** "a, b, c": a list of names or numbers inside a sentence. */
export const listed = (items: readonly (string | number)[]) => items.join(t("common.listSeparator"));

/** A wallet the user picks in a flow, by name. */
export interface WalletChoice {
  walletId: string;
  name: string;
}

/** dash-qt `AskPassphraseDialog`. */
export const WRONG_PASSPHRASE = en.lock.wrongPassphrase;
export const PASSPHRASE_REQUIRED = en.lock.passphraseRequired;

/** Each code with its own copy; `tsc` fails on a catalog key that is no error code. */
type FailureCode = KeyOf<typeof en.onboarding.failure>;
const FAILURE_CODES = [...flatten(en.onboarding.failure).keys()] as FailureCode[] satisfies ErrorCode[];

/** Copy for a failure chosen by its code alone (U-02 D16): the engine's `message` can quote the
 * input (a mnemonic error names the word), so it is never shown. */
const BY_CODE: Partial<Record<ErrorCode, string>> = Object.defineProperty(messageTable("onboarding.failure", FAILURE_CODES), "vault.wrong_passphrase", {
  enumerable: true,
  get: () => t("lock.wrongPassphrase"),
});


export function failureText(e: unknown): string {
  if (!isAppError(e)) return t("onboarding.error.generic");
  if (e.code === "vault.throttled") return tryAgainIn(retryAfterSecs(e));
  if (e.code === "wallet" && /still opening/i.test(e.message)) return t("onboarding.error.walletOpening");
  return BY_CODE[e.code] ?? t("onboarding.error.withCode", { code: e.code });
}

/** "6 minutes", "4 hours": minutes below two hours (rounded up), hours above. */
export function waitText(secs: number): string {
  const minutes = Math.max(1, Math.ceil(secs / 60));
  if (minutes < 120) return t("lock.minutes", { count: minutes });
  return t("lock.hours", { count: Math.ceil(secs / 3600) });
}

/** iOS lockout copy. */
export const tryAgainIn = (secs: number) => t("lock.tryAgainIn", { wait: waitText(secs) });

/** Why a passphrase was not accepted: wrong (with the vault's wait, when it set one), or the
 * wait itself, or another failure. */
export function unlockFailureText(e: unknown): string {
  const wait = retryAfterSecs(e);
  if (hasCode(e, "vault.wrong_passphrase")) return wait > 0 ? `${t("lock.wrongPassphrase")} ${tryAgainIn(wait)}` : t("lock.wrongPassphrase");
  return failureText(e);
}
