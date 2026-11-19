// Max on Send from Shielded: the balance less the engine's fee for a send to the address typed. A quote
// of the whole balance is refused naming the fee the note selection had reached, at first that of the
// two-action minimum; Max quotes the balance less it, and while that is refused naming a larger fee (an
// amount that takes more notes), the balance less that one. Confirm quotes the amount again.

import type { Backend } from "../api/backend";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import { CREDITS_PER_DUFF } from "../api/pending";
import { QuoteLine, SHIELDED_MAX_QUOTES, shieldedSpendMax, type Timers } from "../transfer/model";
import type { ShieldedFlow } from "./flow";

/** The quotes Max reads: a send of the amount to the flow's address as typed when Max is clicked. */
export function sendMaxLine(api: Pick<Backend, "shieldedSendQuote">, flow: ShieldedFlow, timers?: Timers): QuoteLine<ShieldedSpendQuote> {
  return new QuoteLine((duffs) => api.shieldedSendQuote(flow.walletId, flow.to.trim(), String(BigInt(duffs) * CREDITS_PER_DUFF)), timers);
}

/** Max of a send of up to `probe` duffs: the amount the engine's last quote took, or `null` (the line
 * holds the refusal, or the amount or address changed meanwhile). */
export const sendMax = (line: QuoteLine<ShieldedSpendQuote>, probe: number): Promise<number | null> => line.max(probe, shieldedSpendMax, SHIELDED_MAX_QUOTES);
