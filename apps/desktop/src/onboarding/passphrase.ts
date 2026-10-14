// The vault passphrase's rules and strength meter (QT-111, IOS-010).

export const MAX_PASSPHRASE_CHARS = 1024;

export const PassphraseText = {
  empty: "Enter a passphrase.",
  /** dash-qt `AskPassphraseDialog`. */
  mismatch: "The supplied passphrases do not match.",
  tooLong: "The passphrase is longer than 1024 characters.",
  /** dash-qt's encryption warning, with the recovery phrase added. */
  warning:
    "Warning: If you encrypt your wallet and lose your passphrase, you will LOSE ALL OF YOUR DASH! Your recovery phrase still restores the wallet.",
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

export const STRENGTH_NAME: Record<Strength, string> = { none: "", weak: "Weak", fair: "Fair", good: "Good", strong: "Strong" };

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
