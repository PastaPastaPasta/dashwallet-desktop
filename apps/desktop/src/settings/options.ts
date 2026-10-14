// Settings ▸ General (QT-135…141, QT-075): the Swift `OptionsViewModel`'s rules. The display
// settings save as soon as they change (dw-app's file, D2); dust protection is the engine's.

import { DEFAULT_PREFS } from "../app/ui";
import type { DisplayUnit } from "../api/generated/DisplayUnit";
import type { UiSettings } from "../api/generated/UiSettings";
import { UNIT_LABEL } from "../format/display";

export const OptionsText = {
  /** QT-075. */
  dustThresholdInvalid: "The dust threshold must be between 1 and 1,000,000 duffs.",
  settingsNotSaved: "The settings could not be saved.",
  /** QT-141, without "Client will be shut down" (D8). */
  resetTitle: "Confirm options reset",
  resetMessage: "This resets the appearance, units, auto lock and the other display settings to their defaults.",
  resetDone: "The options were reset.",
  /** UX-SPEC §4.12: dash-qt options an SPV wallet has no use for. */
  spvFootnote: [
    "Size of database cache",
    "Number of script verification threads",
    "Prune block storage",
    "Map port using UPnP or NAT-PMP",
    "Allow incoming connections",
    "Show Masternodes tab",
  ],
  unavailable: "Not available yet.",
};

export const UNIT_CHOICES: { unit: DisplayUnit; label: string }[] = (["dash", "milliDash", "microDash", "duffs"] as const).map((unit) => ({
  unit,
  label: UNIT_LABEL[unit],
}));

export const DIGIT_CHOICES = [2, 3, 4, 5, 6, 7, 8];

const MAX_DUST = 1_000_000;

/** The threshold typed in General, or why it is not one (1..=1,000,000 duffs). */
export function dustThreshold(text: string): number | string {
  const t = text.trim().replace(/[,\s]/g, "");
  if (!/^\d+$/.test(t)) return OptionsText.dustThresholdInvalid;
  const n = Number(t);
  return n >= 1 && n <= MAX_DUST ? n : OptionsText.dustThresholdInvalid;
}

/** Reset Options (QT-141, D8): every display setting back to its default. */
export const RESET: UiSettings = DEFAULT_PREFS;
