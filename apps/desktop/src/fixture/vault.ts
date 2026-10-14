// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's recovery phrases (U-02 D19): valid BIP39 phrases drawn from fixed entropy, each
// word once, so the verify chips are unambiguous. `checkMnemonic` knows these and checks the
// words of anything else against the English list; it computes no checksum, so a phrase it does
// not know reads `invalid`. Visual baselines show only these phrases.

import type { MnemonicCheck } from "../api/generated/MnemonicCheck";
import { BIP39_ENGLISH } from "../onboarding/bip39English";
import { normalizePhrase, RESTORE_COUNTS, wordsOf } from "../onboarding/words";

/** `generateMnemonic(12)` and `generateMnemonic(24)`. */
export const CREATED_12 = "stock oven sign razor judge surround shove all tip night corn forget";
export const CREATED_24 =
  "since primary bundle toy dentist light lyrics cattle found camera limb decade project track script galaxy fashion type fun merit reflect depart carpet cross";
/** The `funded` wallet's phrase (Forgot passphrase, Show Recovery Phrase). */
export const MAIN_PHRASE = "address army metal boil club conduct ill trash thing dynamic trumpet prevent";
/** The `two-wallets` scenario's second wallet, and a wallet to restore elsewhere. */
export const SECOND_PHRASE = "enjoy congress layer crop own prefer lecture diagram slight model swarm knock";
/** Passes only Dash Core's legacy checksum (QT-104). */
export const CORE_ONLY_PHRASE = "enjoy congress layer crop own prefer lecture diagram slight model swarm kiwi";

const CHECKSUMS: Record<string, MnemonicCheck["checksum"]> = {
  [CREATED_12]: "valid",
  [CREATED_24]: "valid",
  [MAIN_PHRASE]: "valid",
  [SECOND_PHRASE]: "valid",
  [CORE_ONLY_PHRASE]: "coreOnly",
};

const ENGLISH = new Set(BIP39_ENGLISH);

/** The engine's `check_mnemonic`, as far as the fixture can tell (above). */
export function checkPhrase(phrase: string): MnemonicCheck {
  const normalized = normalizePhrase(phrase);
  const words = wordsOf(normalized);
  const unknownWordIndices = words.flatMap((w, i) => (ENGLISH.has(w) ? [] : [i]));
  const known = CHECKSUMS[normalized];
  return {
    wordCount: words.length,
    unknownWordIndices,
    language: unknownWordIndices.length === 0 ? "english" : null,
    checksum: known && RESTORE_COUNTS.has(words.length) ? known : "invalid",
  };
}

/** A wallet id for a phrase the fixture has no wallet of: 32 bytes from an FNV-1a hash. */
export function walletIdOf(phrase: string): string {
  let h = 0x811c9dc5;
  for (const ch of normalizePhrase(phrase)) h = Math.imul(h ^ ch.codePointAt(0)!, 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0").repeat(8);
}

/** dw-vault `throttle_wait_secs`: the wait after `failed` consecutive failures. */
export const throttleWaitSecs = (failed: number) => (failed < 3 ? 0 : 60 * 6 ** Math.min(failed - 3, 20));
