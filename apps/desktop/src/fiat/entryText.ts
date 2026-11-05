// What amount entry in fiat says (fiat/entry.tsx), apart from the UI so specs can read it.

import { minorUnits } from "../format/fiat";

export const ENTRY_TEXT = {
  notFresh: "Prices are over 30 minutes old: enter the amount in DASH.",
  unavailable: "Prices unavailable: enter the amount in DASH.",
  backToDash: "Prices are no longer current, so the amount is in DASH again.",
  tooLarge: "That amount is too large to convert to DASH.",
  invalid: (currency: string) => `Enter an amount in ${currency}, for example ${minorUnits(currency) === 0 ? "1250" : "12.50"}.`,
};
