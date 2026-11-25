import { errorMessage, hasCode } from "../api/backend";
import {
  isStoreRefusedCode,
  isStoreRestoreCode,
  isStoreRetryCode,
  storageInUse,
  storeRefusal,
  storeRestore,
  storeRetry,
  type StoreRefusedData,
  type StoreRestoreData,
  type StoreRetryData,
} from "../api/storeOpen";
import { t } from "../i18n";

/** What shows instead of the wallet when opening the network fails:
 * - `refused`: the engine refused the wallet data and changed nothing (`storage_refused`); its own screen;
 * - `inUse`: the session before still holds it (`storage_in_use`); its own screen, with Try again only when
 *   `retryable`;
 * - `restore`: a restore the user asked for stopped the open (`storage_restore`); its state's screen;
 * - `retry`: the upgrade stopped on the machine and runs again at the next start (`storage_retry`); its
 *   own screen, never the earlier version;
 * - `message`: the splash's text, with a retry unless the file is from a newer build (`storage_too_new`,
 *   DP3-04b D35), which opens only with that build. */
export type OpenFailure =
  | { kind: "message"; text: string; retry: boolean }
  | { kind: "refused"; refusal: StoreRefusedData }
  | { kind: "inUse"; retryable: boolean }
  | { kind: "restore"; restore: StoreRestoreData }
  | { kind: "retry"; retry: StoreRetryData };

export function openFailure(e: unknown): OpenFailure {
  const refusal = storeRefusal(e);
  if (refusal) return { kind: "refused", refusal };
  const restore = storeRestore(e);
  if (restore) return { kind: "restore", restore };
  const retry = storeRetry(e);
  if (retry) return { kind: "retry", retry };
  const inUse = storageInUse(e);
  if (inUse) return { kind: "inUse", retryable: inUse.retryable };
  if (hasCode(e, "storage_too_new")) {
    return {
      kind: "message",
      text: t("app.openFailure.tooNew", { error: errorMessage(e) }),
      retry: false,
    };
  }
  // A refusal, restore or upgrade stop without its data (a malformed DTO) is the engine's message alone:
  // retrying in this process can't help.
  const unscreened = isStoreRefusedCode(e) || isStoreRestoreCode(e) || isStoreRetryCode(e);
  const own = (e as { message?: unknown } | null)?.message;
  const message = unscreened && typeof own === "string" ? own : errorMessage(e);
  return { kind: "message", text: t("app.openFailure.failed", { error: message }), retry: !unscreened };
}
