// Max on Send from Shielded: the balance less the engine's fee for a send to the address typed. The
// engine's fee only grows with the amount (it is priced on the notes the amount takes, largest first),
// so the second quote, of the balance less the first quote's fee, takes it. Confirm quotes it again.

import type { Backend } from "../api/backend";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import { CREDITS_PER_DUFF } from "../api/pending";
import { QuoteLine, shieldedSpendMax, type Timers } from "../transfer/model";
import type { ShieldedFlow } from "./flow";

/** The quotes Max reads: a send of the amount to the flow's address as typed when Max is clicked. */
export function sendMaxLine(api: Pick<Backend, "shieldedSendQuote">, flow: ShieldedFlow, timers?: Timers): QuoteLine<ShieldedSpendQuote> {
  return new QuoteLine((duffs) => api.shieldedSendQuote(flow.walletId, flow.to.trim(), String(BigInt(duffs) * CREDITS_PER_DUFF)), timers);
}

/** Max of a send of up to `probe` duffs: the amount the engine's second quote took, or `null` (the
 * line holds the refusal, or the amount or address changed meanwhile). */
export const sendMax = (line: QuoteLine<ShieldedSpendQuote>, probe: number): Promise<number | null> => line.max(probe, shieldedSpendMax);
