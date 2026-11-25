import { errorMessage, hasCode } from "../api/backend";
import { isStoreRefusedCode, storageInUse, storeRefusal, type StoreRefusedData } from "../api/storeOpen";
import { t } from "../i18n";

/** What shows instead of the wallet when opening the network fails:
 * - `refused`: the engine refused the wallet data and changed nothing (`storage_refused`); its own screen;
 * - `inUse`: the session before still holds it (`storage_in_use`); its own screen, with Try again only when
 *   `retryable`;
 * - `message`: the splash's text, with a retry unless the file is from a newer build (`storage_too_new`,
 *   DP3-04b D35), which opens only with that build. */
export type OpenFailure = { kind: "message"; text: string; retry: boolean } | { kind: "refused"; refusal: StoreRefusedData } | { kind: "inUse"; retryable: boolean };

export function openFailure(e: unknown): OpenFailure {
  const refusal = storeRefusal(e);
  if (refusal) return { kind: "refused", refusal };
  const inUse = storageInUse(e);
  if (inUse) return { kind: "inUse", retryable: inUse.retryable };
  if (hasCode(e, "storage_too_new")) {
    return {
      kind: "message",
      text: t("app.openFailure.tooNew", { error: errorMessage(e) }),
      retry: false,
    };
  }
  // A refusal without its screen (a malformed DTO) is the engine's message alone: retrying can't help.
  return { kind: "message", text: t("app.openFailure.failed", { error: errorMessage(e) }), retry: !isStoreRefusedCode(e) };
}
