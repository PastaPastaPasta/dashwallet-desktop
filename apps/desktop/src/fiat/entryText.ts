// What amount entry in fiat says (fiat/entry.tsx), apart from the UI so specs can read it.

import { minorUnits } from "../format/fiat";
import { t } from "../i18n";

export const ENTRY_TEXT = {
  get notFresh() {
    return t("app.fiat.notFresh");
  },
  get unavailable() {
    return t("app.fiat.unavailable");
  },
  get backToDash() {
    return t("app.fiat.backToDash");
  },
  notKept: (currency: string) => t("app.fiat.notKept", { currency }),
  get tooLarge() {
    return t("app.fiat.tooLarge");
  },
  invalid: (currency: string) => t("app.fiat.invalid", { currency, example: minorUnits(currency) === 0 ? "1250" : "12.50" }),
};
