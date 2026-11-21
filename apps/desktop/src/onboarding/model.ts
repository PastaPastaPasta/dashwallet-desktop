// The base of the U-02 view models: plain classes (no React) that announce changes, read by the
// screens through `useModel`. Secrets stay in private fields; nothing here persists.

import { useSyncExternalStore } from "react";
import { hasCode, isAppError, retryAfterSecs } from "../api/backend";
import type { ErrorCode } from "../api/generated/ErrorCode";
import { t } from "../i18n";
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

/** A wallet the user picks in a flow, by name. */
export interface WalletChoice {
  walletId: string;
  name: string;
}

/** dash-qt `AskPassphraseDialog`. */
export const WRONG_PASSPHRASE = en.lock.wrongPassphrase;
export const PASSPHRASE_REQUIRED = en.lock.passphraseRequired;
export const INVALID_PHRASE = en.components.failure.code.invalid_mnemonic;

/** Failure codes the renderer has its own words for, under `components.failure.code` (U-02 D16):
 * the engine's `message` can quote the input (a mnemonic error names the word), so it is never shown. */
const BY_CODE: ReadonlySet<string> = new Set<ErrorCode>([...flatten(en.components.failure.code).keys()] as CodeKey[]);
type CodeKey = KeyOf<typeof en.components.failure.code>;

export function failureText(e: unknown): string {
  if (!isAppError(e)) return t("components.failure.generic");
  if (e.code === "vault.throttled") return tryAgainIn(retryAfterSecs(e));
  if (e.code === "wallet" && /still opening/i.test(e.message)) return t("components.failure.walletOpening");
  if (e.code === "vault.wrong_passphrase") return t("lock.wrongPassphrase");
  return BY_CODE.has(e.code) ? t(`components.failure.code.${e.code as CodeKey}`) : t("components.failure.unknownCode", { code: e.code });
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
