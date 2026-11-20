// Settings ▸ General (QT-135…141, QT-075): the Swift `OptionsViewModel`'s rules. The display
// settings save as soon as they change (dw-app's file, D2); dust protection is the engine's.

import type { DisplayUnit } from "../api/generated/DisplayUnit";
import { UNIT_LABEL } from "../format/display";
import { t } from "../i18n";

export const OptionsText = {
  /** QT-075. */
  get dustThresholdInvalid() {
    return t("settings.general.dustInvalid");
  },
  get settingsNotSaved() {
    return t("settings.notSaved");
  },
  /** QT-141, without "Client will be shut down" (D8). */
  get resetTitle() {
    return t("settings.general.resetTitle");
  },
  get resetMessage() {
    return t("settings.general.resetMessage");
  },
  get resetDone() {
    return t("settings.general.resetDone");
  },
  /** UX-SPEC §4.12: dash-qt options an SPV wallet has no use for. */
  spvFootnote: [
    "Size of database cache",
    "Number of script verification threads",
    "Prune block storage",
    "Map port using UPnP or NAT-PMP",
    "Allow incoming connections",
    "Show Masternodes tab",
  ],
  get unavailable() {
    return t("settings.general.unavailable");
  },
};

export const UNIT_CHOICES: { unit: DisplayUnit; label: string }[] = (["dash", "milliDash", "microDash", "duffs"] as const).map((unit) => ({
  unit,
  label: UNIT_LABEL[unit],
}));

const MAX_DUST = 1_000_000;
/** The threshold General offers when protection is turned on without one typed. */
export const DEFAULT_DUST = 1000;

/** The threshold typed in General, or why it is not one (1..=1,000,000 duffs). */
export function dustThreshold(text: string): number | string {
  const t = text.trim().replace(/[,\s]/g, "");
  if (!/^\d+$/.test(t)) return OptionsText.dustThresholdInvalid;
  const n = Number(t);
  return n >= 1 && n <= MAX_DUST ? n : OptionsText.dustThresholdInvalid;
}
