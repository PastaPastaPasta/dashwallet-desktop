import { errorMessage, hasCode } from "../api/backend";

/** What the splash says when opening the network fails, and whether trying again can help.
 * A wallet file a newer build wrote (`storage_too_new`, DP3-04b D35) opens only with that
 * build, so it gets its own text, the engine's versions for support, and no retry. */
export type OpenFailure = { text: string; retry: boolean };

export function openFailure(e: unknown): OpenFailure {
  if (hasCode(e, "storage_too_new")) {
    return {
      text:
        "This wallet file was written by a newer version of Dash Wallet. Open it with that version; " +
        `nothing was changed. (${errorMessage(e)})`,
      retry: false,
    };
  }
  return { text: `Dash Wallet could not open the wallet: ${errorMessage(e)}`, retry: true };
}
