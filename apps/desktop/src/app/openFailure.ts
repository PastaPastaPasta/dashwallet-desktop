import { errorMessage, hasCode } from "../api/backend";
import { isNotPrivateCode, isStoreRefusedCode, isStoreRestoreCode, storageInUse, storeNotPrivate, storeRefusal, storeRestore, type NotPrivateData, type StoreRefusedData, type StoreRestoreData } from "../api/storeOpen";
import { t } from "../i18n";

/** What shows instead of the wallet when opening the network fails:
 * - `refused`: the engine refused the wallet data and changed nothing (`storage_refused`); its own screen;
 * - `inUse`: the session before still holds it (`storage_in_use`); its own screen, with Try again only when
 *   `retryable`;
 * - `restore`: a restore the user asked for stopped the open (`storage_restore`); its state's screen;
 * - `notPrivate`: others can change the data folder, so the engine read nothing below it (`storage_not_private`);
 *   its own screen, whose Try again opens again once the user changed the folder's permissions;
 * - `message`: the splash's text, with a retry unless the file is from a newer build (`storage_too_new`,
 *   DP3-04b D35), which opens only with that build. */
export type OpenFailure =
  | { kind: "message"; text: string; retry: boolean }
  | { kind: "refused"; refusal: StoreRefusedData }
  | { kind: "inUse"; retryable: boolean }
  | { kind: "restore"; restore: StoreRestoreData }
  | { kind: "notPrivate"; notPrivate: NotPrivateData };

export function openFailure(e: unknown): OpenFailure {
  const notPrivate = storeNotPrivate(e);
  if (notPrivate) return { kind: "notPrivate", notPrivate };
  const refusal = storeRefusal(e);
  if (refusal) return { kind: "refused", refusal };
  const restore = storeRestore(e);
  if (restore) return { kind: "restore", restore };
  const inUse = storageInUse(e);
  if (inUse) return { kind: "inUse", retryable: inUse.retryable };
  if (hasCode(e, "storage_too_new")) {
    return {
      kind: "message",
      text: t("app.openFailure.tooNew", { error: errorMessage(e) }),
      retry: false,
    };
  }
  // A refusal or restore failure without its data (a malformed DTO) is the engine's message alone:
  // retrying can't help. A data folder others can change keeps the retry: it opens once the user fixed it.
  const unscreened = isStoreRefusedCode(e) || isStoreRestoreCode(e);
  const own = (e as { message?: unknown } | null)?.message;
  const message = (unscreened || isNotPrivateCode(e)) && typeof own === "string" ? own : errorMessage(e);
  return { kind: "message", text: t("app.openFailure.failed", { error: message }), retry: !unscreened };
}
