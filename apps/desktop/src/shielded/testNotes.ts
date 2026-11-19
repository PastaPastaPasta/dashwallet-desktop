// Tests: a spend quote as the engine answers it over a set of notes (`send_quote` / `unshield_quote` →
// `select_notes_with_fee`), priced per action by the fixture's model of the library's note selection.

import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import { SHIELDED_CODES } from "../api/pending";
import { noteSelectionFee } from "../fixture/shielded";
import { appError } from "../onboarding/testWorld";

/** The balance's notes, in credits, by how many there are: 1.25 tDASH each time. */
export const NOTE_SETS = {
  2: [100_000_000_000n, 25_000_000_000n],
  3: [100_000_000_000n, 20_000_000_000n, 5_000_000_000n],
  5: [60_000_000_000n, 30_000_000_000n, 20_000_000_000n, 10_000_000_000n, 5_000_000_000n],
} as const;

export const NOTES_BALANCE = 125_000_000_000n;

export function engineSpendQuote(notes: readonly bigint[], credits: string): ShieldedSpendQuote {
  const c = BigInt(credits);
  let fee: bigint;
  try {
    fee = noteSelectionFee(notes, c);
  } catch (e) {
    const available = String(notes.reduce((n, v) => n + v, 0n));
    throw appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: String((e as { needed: bigint }).needed), available });
  }
  const total = String(c + fee);
  return { credits, feeCredits: String(fee), totalCredits: total, expectedDuffs: null, grant: { maxDuffs: 0, maxCredits: total } };
}
