// The send flow's close path, for the send screen (U-07) to close drafts through. `closeDraft`
// only stops later calls on the draft: a `draftBroadcast` already running keeps going and the
// transaction may still reach the network (E0-13 D8). The screen knows its own broadcast calls
// and, when one is running, shows DEC-67's line (E0-04 §16.10) instead of implying the send
// was stopped.

import { closeDraft } from "./generated/commands";

/** DEC-67's copy for a signature already released (E0-04 §16.10). */
export const SIGNED_BEFORE_LOCK = "Lock stops new signatures; a transaction already signed may still be sent";

/** Closes a draft; `warning` is the copy to show when a broadcast of it is still running. */
export async function closeSendDraft(draft: number, broadcastInFlight: boolean): Promise<{ closed: boolean; warning: string | null }> {
  const closed = await closeDraft(draft);
  return { closed, warning: broadcastInFlight ? SIGNED_BEFORE_LOCK : null };
}
