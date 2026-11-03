// `rescan_in_progress` from a spend (0.1.x-bal §1): a replay of the filter scan runs, or a rescan
// the user stopped part way must run again before the wallet's coins can be trusted.
import { hasCode } from "../api/backend";

export const RESCAN_RUNNING = "The wallet is rescanning. Wait until it has finished, then try again.";
export const RESCAN_INTERRUPTED = "The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.";

/** `text` for `e`, unless it is `rescan_in_progress`: then the running text while a rescan runs, else the interrupted one. */
export async function rescanAware(e: unknown, text: string, progress: () => Promise<unknown>): Promise<string> {
  if (!hasCode(e, "rescan_in_progress")) return text;
  return (await progress().catch(() => true)) ? RESCAN_RUNNING : RESCAN_INTERRUPTED;
}
