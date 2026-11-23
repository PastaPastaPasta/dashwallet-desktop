import { errorMessage, hasCode } from "../api/backend";
import { t } from "../i18n";

/** What the splash says when opening the network fails, and whether trying again can help.
 * A wallet file a newer build wrote (`storage_too_new`, DP3-04b D35) opens only with that
 * build, so it gets its own text, the engine's versions for support, and no retry. */
export type OpenFailure = { text: string; retry: boolean };

export function openFailure(e: unknown): OpenFailure {
  if (hasCode(e, "storage_too_new")) {
    return {
      text: t("app.openFailure.tooNew", { error: errorMessage(e) }),
      retry: false,
    };
  }
  return { text: t("app.openFailure.failed", { error: errorMessage(e) }), retry: true };
}
