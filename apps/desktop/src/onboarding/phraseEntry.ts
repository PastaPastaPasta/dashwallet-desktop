// A typed or pasted recovery phrase, checked live by the engine (IOS-007, U-02 D7): restore and
// Forgot passphrase share it.

import type { Backend } from "../api/backend";
import type { MnemonicCheck } from "../api/generated/MnemonicCheck";
import { t } from "../i18n";
import { failureText, listed } from "./model";
import { normalizePhrase, partialWord, RESTORE_COUNTS, suggestions } from "./words";

export const PhraseText = {
  get invalid() {
    return t("onboarding.failure.invalid_mnemonic");
  },
  get wordCount() {
    return t("onboarding.phrase.wordCount");
  },
  get coreOnly() {
    return t("onboarding.phrase.coreOnly");
  },
  unknownWords(positions: number[]): string {
    const words = listed(positions.map((p) => p + 1));
    return positions.length === 1 ? t("onboarding.phrase.unknownWord", { words }) : t("onboarding.phrase.unknownWords", { words });
  },
};

/** What is wrong with a checked phrase, or `null` when it can be restored. */
export function phraseProblem(check: MnemonicCheck): string | null {
  if (check.unknownWordIndices.length > 0) return PhraseText.unknownWords(check.unknownWordIndices);
  if (!RESTORE_COUNTS.has(check.wordCount)) return PhraseText.wordCount;
  if (check.checksum === "invalid") return PhraseText.invalid;
  return null;
}

export class PhraseEntry {
  check: MnemonicCheck | null = null;
  problem: string | null = null;
  suggestions: string[] = [];
  #phrase: string | null = null;
  #generation = 0;
  /** The edit whose answer `check` holds. */
  #answered = 0;

  constructor(private readonly checkMnemonic: Backend["checkMnemonic"]) {}

  /** The normalized phrase once the engine accepted it (a valid or Dash Core-only checksum). */
  get phrase(): string | null {
    return this.valid ? this.#phrase : null;
  }

  /** Not while a newer edit's answer is still to come: the shown one may no longer hold. */
  get valid(): boolean {
    return !this.pending && this.check !== null && this.problem === null;
  }

  /** The last edit is still being checked. */
  get pending(): boolean {
    return this.#answered !== this.#generation;
  }

  get coreOnly(): boolean {
    return this.check?.checksum === "coreOnly";
  }

  /** The problem to show. While the last word is still being typed (`typing`), only an unknown
   * word before it counts: the length and the checksum are not wrong yet. */
  shownProblem(typing: boolean): string | null {
    const check = this.check;
    if (!typing || !check || !this.problem) return this.problem;
    const earlier = check.unknownWordIndices.filter((i) => i < check.wordCount - 1);
    return earlier.length > 0 ? PhraseText.unknownWords(earlier) : null;
  }

  /** Checks `text`; `changed` runs once the answer for this edit is in. A later edit's answer
   * wins: earlier ones are dropped when they arrive. */
  async update(text: string, changed: () => void): Promise<void> {
    this.suggestions = suggestions(partialWord(text));
    const generation = ++this.#generation;
    const normalized = normalizePhrase(text);
    if (!normalized) {
      this.clear();
      changed();
      return;
    }
    changed();
    try {
      const check = await this.checkMnemonic(normalized);
      if (generation !== this.#generation) return;
      this.#answered = generation;
      this.#phrase = normalized;
      this.check = check;
      this.problem = phraseProblem(check);
    } catch (e) {
      if (generation !== this.#generation) return;
      this.#answered = generation;
      this.#phrase = null;
      this.check = null;
      this.problem = failureText(e);
    }
    changed();
  }

  clear() {
    this.#answered = ++this.#generation;
    this.#phrase = null;
    this.check = null;
    this.problem = null;
    this.suggestions = [];
  }
}
