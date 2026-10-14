// Recovery-phrase helpers that need no engine: word counts, normalization and suggestions
// (IOS-002, IOS-007). The checksum and the word lists of every language are the engine's
// (`checkMnemonic`).

import { BIP39_ENGLISH } from "./bip39English";

/** Word counts a recovery phrase may have (IOS-007, QT-104). */
export const RESTORE_COUNTS: ReadonlySet<number> = new Set([12, 15, 18, 21, 24]);
/** Word counts offered when creating a wallet (IOS-002). */
export const CREATE_COUNTS = [12, 24] as const;
export type CreateCount = (typeof CREATE_COUNTS)[number];

/** Lower case, words joined by single spaces. */
export function normalizePhrase(text: string): string {
  return text.toLowerCase().split(/\s+/u).filter(Boolean).join(" ");
}

/** The words of a normalized phrase. */
export const wordsOf = (phrase: string): string[] => (phrase ? phrase.split(" ") : []);

/** English words starting with `prefix` (case-insensitive), at most `limit`. Empty for an empty
 * prefix or when the prefix is already the only match. */
export function suggestions(prefix: string, limit = 5): string[] {
  const lower = prefix.toLowerCase();
  if (!lower) return [];
  const found: string[] = [];
  for (const w of BIP39_ENGLISH) {
    if (w.startsWith(lower)) found.push(w);
    if (found.length === limit) break;
  }
  return found.length === 1 && found[0] === lower ? [] : found;
}

/** The word being typed: the last one, unless the text ends in whitespace. */
export function partialWord(text: string): string {
  if (!text || /\s$/u.test(text)) return "";
  return text.split(/\s+/u).at(-1) ?? "";
}

/** `text` with its last, partly typed word replaced by `word` and a space after it. */
export function completeWord(text: string, word: string): string {
  const partial = partialWord(text);
  return `${text.slice(0, text.length - partial.length)}${word} `;
}
