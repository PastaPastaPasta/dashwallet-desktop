// The vault passphrase's rules and strength meter (QT-111, IOS-010).

import { t } from "../i18n";

export const MAX_PASSPHRASE_CHARS = 1024;

export const PassphraseText = {
  get empty() {
    return t("onboarding.passphrase.empty");
  },
  /** dash-qt `AskPassphraseDialog`. */
  get mismatch() {
    return t("onboarding.passphrase.mismatch");
  },
  get tooLong() {
    return t("onboarding.passphrase.tooLong");
  },
  get warning() {
    return t("onboarding.passphrase.warning");
  },
};

/** Why `text` cannot be the new passphrase, or `null` when it can. Characters are counted as
 * the user sees them (code points), as the Swift app did. */
export function passphraseProblem(text: string, confirmation: string): string | null {
  if (!text) return PassphraseText.empty;
  if (text !== confirmation) return PassphraseText.mismatch;
  if ([...text].length > MAX_PASSPHRASE_CHARS) return PassphraseText.tooLong;
  return null;
}

export const STRENGTHS = ["none", "weak", "fair", "good", "strong"] as const;
export type Strength = (typeof STRENGTHS)[number];

export const STRENGTH_NAME: Readonly<Record<Strength, string>> = {
  none: "",
  get weak() {
    return t("onboarding.passphrase.strength.weak");
  },
  get fair() {
    return t("onboarding.passphrase.strength.fair");
  },
  get good() {
    return t("onboarding.passphrase.strength.good");
  },
  get strong() {
    return t("onboarding.passphrase.strength.strong");
  },
};

/** A rough meter that only guides; the vault's Argon2id cost is the protection. Under 8
 * characters is weak; each character class (lower, upper, digit, other) and each 4 characters
 * beyond 8 (at most 3) add a point. */
export function passphraseStrength(text: string): Strength {
  const chars = [...text];
  if (chars.length === 0) return "none";
  if (chars.length < 8) return "weak";
  const classes = [/\p{Ll}/u, /\p{Lu}/u, /\p{N}/u, /[^\p{L}\p{N}]/u].filter((re) => re.test(text)).length;
  const score = classes + Math.min(3, Math.floor((chars.length - 8) / 4));
  if (score <= 1) return "weak";
  if (score <= 3) return "fair";
  return score === 4 ? "good" : "strong";
}
