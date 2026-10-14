// Create and restore (IOS-002, IOS-004, IOS-007, IOS-010, QT-102…105, QT-111): the Swift
// `OnboardingViewModel`, ported, in U-02's order (D2): create asks for the vault passphrase
// before the phrase and adds the wallet at the last correct chip; restore asks for it last.
// Nothing is stored before the wallet is added, and the phrase is dropped once it is.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { LockStateName } from "../api/generated/LockStateName";
import type { MnemonicCheck } from "../api/generated/MnemonicCheck";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { birthHeightFor } from "./birthHeight";
import { failureText, Model, PASSPHRASE_REQUIRED, unlockFailureText } from "./model";
import { passphraseProblem, passphraseStrength, type Strength } from "./passphrase";
import { PhraseEntry } from "./phraseEntry";
import { CREATE_COUNTS, wordsOf, type CreateCount } from "./words";

export type OnboardingStep =
  | "welcome"
  /** The vault passphrase for a network without a vault (QT-111, IOS-010). */
  | "choosePassphrase"
  /** The vault is locked or unlocked for mixing only: its passphrase comes first (D3). */
  | "unlockVault"
  /** The new phrase is on screen (IOS-003). */
  | "showPhrase"
  /** Word chips (IOS-004). */
  | "verifyPhrase"
  | "restorePhrase"
  /** BIP39 passphrase, Dash Core compatibility, scan start (QT-104, QT-105). */
  | "restoreOptions"
  | "working"
  | "done"
  | "failed";

export type OnboardingFlow = "create" | "restore";

/** One word chip on the verify screen. */
export interface VerifyChip {
  id: number;
  word: string;
  used: boolean;
}

export interface RestoreOptions {
  /** `null`: the engine's default, its tip, right for a new wallet only. */
  birthHeight: number | null;
  coreCompat: boolean;
}

export const OnboardingText = {
  birthDateInFuture: "The wallet creation date cannot be in the future.",
};

type Api = Pick<Backend, "lockStatus" | "unlock" | "createVault" | "generateMnemonic" | "checkMnemonic" | "importWallet">;

/** A number in [0, 1) from the system's CSPRNG. */
const systemRandom = () => crypto.getRandomValues(new Uint32Array(1))[0] / 2 ** 32;

/** Vault states that need the passphrase before a wallet can be added. */
const NEEDS_UNLOCK: ReadonlySet<LockStateName> = new Set(["locked", "unlockedMixingOnly"]);

export class OnboardingModel extends Model {
  step: OnboardingStep = "welcome";
  flow: OnboardingFlow | null = null;
  vaultState: LockStateName | null = null;

  // Create.
  wordCount: CreateCount = 12;
  /** Display only: filled while the phrase step shows, cleared when it ends. */
  phraseWords: string[] = [];
  /** Phrase positions (0-based) to confirm, ascending. */
  verifyChallenge: number[] = [];
  verifyChips: VerifyChip[] = [];
  verifiedCount = 0;
  verifyMistake = false;

  // The vault passphrase.
  passphraseStrength: Strength = "none";
  passphraseError: string | null = null;
  unlockError: string | null = null;

  // Restore.
  readonly entry: PhraseEntry;
  birthDate: Date | null = null;
  birthDateError: string | null = null;
  options: RestoreOptions = { birthHeight: null, coreCompat: false };

  walletId: string | null = null;
  failure: string | null = null;

  #phrase: string | null = null;
  #bip39: string | null = null;
  #vaultPassphrase: string | null = null;
  #skipEncryption = false;
  /** What a successful unlock goes on with. */
  #afterUnlock: "create" | "restore" | "finish" = "create";
  readonly #random: () => number;
  readonly #now: () => Date;

  constructor(
    private readonly api: Api,
    private readonly network: NetworkInfo["kind"],
    opts: { random?: () => number; now?: () => Date } = {},
  ) {
    super();
    this.entry = new PhraseEntry(api.checkMnemonic);
    this.#random = opts.random ?? systemRandom;
    this.#now = opts.now ?? (() => new Date());
  }

  get isVerified(): boolean {
    return this.verifyChallenge.length > 0 && this.verifiedCount === this.verifyChallenge.length;
  }

  /** A phrase is on screen: the window is kept out of screen capture (D15). */
  get phraseVisible(): boolean {
    return this.step === "showPhrase" || this.step === "verifyPhrase";
  }

  get restoreCheck(): MnemonicCheck | null {
    return this.entry.check;
  }

  get restoreProblem(): string | null {
    return this.entry.problem;
  }

  get wordSuggestions(): string[] {
    return this.entry.suggestions;
  }

  get coreOnlyWarning(): boolean {
    return this.entry.coreOnly;
  }

  get canContinueRestore(): boolean {
    return this.entry.valid;
  }

  setWordCount(count: number) {
    if (!CREATE_COUNTS.includes(count as CreateCount)) return;
    this.wordCount = count as CreateCount;
    this.changed();
  }

  // Create

  /** Starts a new wallet: the passphrase or the unlock first when the vault needs one, then a
   * fresh phrase. Nothing is stored yet. */
  async startCreate() {
    this.#clear();
    this.flow = "create";
    // A new wallet has no history: `null` lets the engine start at its tip (review H-5).
    this.options = { birthHeight: null, coreCompat: false };
    if (!(await this.#readVault())) return;
    if (this.vaultState === "noVault") this.#go("choosePassphrase");
    else if (this.#needsUnlock("create")) this.#go("unlockVault");
    else await this.#generate();
  }

  /** The user wrote the phrase down: hide it and draw the verify challenge (D4). */
  confirmWrittenDown() {
    if (this.step !== "showPhrase" || !this.#phrase) return;
    const words = wordsOf(this.#phrase);
    const positions = this.#shuffled(words.map((_, i) => i));
    const size = Math.min(4, words.length);
    this.verifyChallenge = positions.slice(0, size).sort((a, b) => a - b);
    const asked = new Set(this.verifyChallenge.map((i) => words[i]));
    const decoys = [...new Set(positions.slice(size).map((i) => words[i]).filter((w) => !asked.has(w)))].sort().slice(0, 2);
    const chipWords = this.#shuffled([...this.verifyChallenge.map((i) => words[i]), ...decoys]);
    this.verifyChips = chipWords.map((word, id) => ({ id, word, used: false }));
    this.verifiedCount = 0;
    this.verifyMistake = false;
    this.phraseWords = [];
    this.#go("verifyPhrase");
  }

  /** Whether `word` is the phrase's word at `index` (0-based). */
  verify(word: string, index: number): boolean {
    const words = this.#phrase ? wordsOf(this.#phrase) : [];
    return index >= 0 && index < words.length && words[index] === word;
  }

  /** Picks a chip for the next position. A wrong one marks the step and does not advance; the
   * last right one adds the wallet. */
  async select(chipId: number) {
    if (this.step !== "verifyPhrase" || this.isVerified) return;
    const chip = this.verifyChips.find((c) => c.id === chipId && !c.used);
    if (!chip) return;
    if (this.verify(chip.word, this.verifyChallenge[this.verifiedCount])) {
      this.verifyChips = this.verifyChips.map((c) => (c === chip ? { ...c, used: true } : c));
      this.verifiedCount++;
      this.verifyMistake = false;
      this.changed();
      if (this.isVerified) await this.finish();
    } else {
      this.verifyMistake = true;
      this.changed();
    }
  }

  // The vault passphrase

  updatePassphraseDraft(text: string) {
    this.passphraseStrength = passphraseStrength(text);
    this.passphraseError = null;
    this.changed();
  }

  /** Takes the new vault's passphrase; `false` with `passphraseError` when it is refused. */
  async submitPassphrase(text: string, confirmation: string): Promise<boolean> {
    if (this.step !== "choosePassphrase") return false;
    const problem = passphraseProblem(text, confirmation);
    if (problem) {
      this.passphraseError = problem;
      this.changed();
      return false;
    }
    this.#vaultPassphrase = text;
    this.#skipEncryption = false;
    await this.#afterPassphrase();
    return true;
  }

  /** No passphrase: the vault's key goes to the OS secret store (D17). */
  async skipEncryption() {
    if (this.step !== "choosePassphrase") return;
    this.#vaultPassphrase = null;
    this.#skipEncryption = true;
    await this.#afterPassphrase();
  }

  /** Unlocks the existing vault fully, then goes on. A wrong passphrase stays here. */
  async unlockVault(passphrase: string) {
    if (this.step !== "unlockVault") return;
    if (!passphrase) {
      this.unlockError = PASSPHRASE_REQUIRED;
      this.changed();
      return;
    }
    try {
      this.vaultState = (await this.api.unlock(passphrase)).state;
    } catch (e) {
      this.unlockError = unlockFailureText(e);
      this.changed();
      return;
    }
    this.unlockError = null;
    if (this.#afterUnlock === "create") await this.#generate();
    else if (this.#afterUnlock === "restore") this.#go("restorePhrase");
    else await this.finish();
  }

  // Restore

  async startRestore() {
    this.#clear();
    this.flow = "restore";
    // Scan from genesis by default (review H-5); no lookahead: the engine's default.
    this.options = { birthHeight: 0, coreCompat: false };
    this.birthDate = null;
    this.birthDateError = null;
    if (!(await this.#readVault())) return;
    this.#go(this.#needsUnlock("restore") ? "unlockVault" : "restorePhrase");
  }

  /** Checks the typed phrase (D7). */
  async updateRestoreText(text: string) {
    await this.entry.update(text, () => {
      if (this.entry.coreOnly) this.options = { ...this.options, coreCompat: true };
      this.changed();
    });
  }

  continueRestore() {
    if (this.step !== "restorePhrase" || !this.entry.valid) return;
    this.#go("restoreOptions");
  }

  /** The BIP39 passphrase ("25th word"); empty means none. */
  setBip39Passphrase(text: string) {
    this.#bip39 = text || null;
  }

  /** Dash Core compatibility (QT-104); stays on for a phrase that passes only Core's checksum. */
  setCoreCompatible(on: boolean) {
    this.options = { ...this.options, coreCompat: on || this.entry.coreOnly };
    this.changed();
  }

  /** Scan start from the wallet's creation date; `null` scans from genesis. */
  setBirthDate(date: Date | null) {
    this.birthDateError = null;
    if (!date) {
      this.birthDate = null;
      this.options = { ...this.options, birthHeight: 0 };
    } else if (date > this.#now()) {
      this.birthDateError = OnboardingText.birthDateInFuture;
    } else {
      this.birthDate = date;
      this.options = { ...this.options, birthHeight: birthHeightFor(date, this.network) };
    }
    this.changed();
  }

  /** Scan start as a block height; `null` scans from genesis. */
  setBirthHeight(height: number | null) {
    this.birthDate = null;
    this.birthDateError = null;
    this.options = { ...this.options, birthHeight: height ?? 0 };
    this.changed();
  }

  async finishRestore() {
    if (this.step !== "restoreOptions" || this.birthDateError !== null) return;
    if (this.vaultState === "noVault") {
      this.passphraseStrength = "none";
      this.#go("choosePassphrase");
    } else await this.finish();
  }

  // Finish

  /** Creates the vault when there is none, adds the wallet, and drops every secret. */
  async finish() {
    const phrase = this.flow === "restore" ? this.entry.phrase : this.#phrase;
    if (!phrase) return;
    this.#go("working");
    try {
      if (this.vaultState === "noVault") {
        this.vaultState = (await this.api.createVault(this.#skipEncryption ? null : this.#vaultPassphrase)).state;
        this.#vaultPassphrase = null;
      }
      const id = await this.api.importWallet(phrase, this.#bip39, {
        name: null,
        birthHeight: this.options.birthHeight,
        coreCompat: this.options.coreCompat,
        lookahead: null,
      });
      this.#clear();
      this.walletId = id;
      this.#go("done");
    } catch (e) {
      if (hasCode(e, "vault.locked") || hasCode(e, "vault.mixing_only")) {
        this.vaultState = "locked";
        this.unlockError = null;
        this.#afterUnlock = "finish";
        this.#go("unlockVault");
      } else {
        this.failure = failureText(e);
        this.#go("failed");
      }
    }
  }

  /** Returns to the previous screen; from a failure, to where the user can try again. */
  back() {
    switch (this.step) {
      case "showPhrase":
      case "restorePhrase":
        this.cancel();
        return;
      case "verifyPhrase":
        this.#showPhrase();
        return;
      case "restoreOptions":
        this.#go("restorePhrase");
        return;
      case "choosePassphrase":
        if (this.flow === "restore") this.#go("restoreOptions");
        else this.cancel();
        return;
      case "unlockVault":
        this.unlockError = null;
        if (this.#afterUnlock === "finish" && this.flow === "restore") this.#go("restoreOptions");
        else if (this.#afterUnlock === "finish") this.#showPhrase();
        else this.cancel();
        return;
      case "failed":
        if (!(this.flow === "restore" ? this.entry.phrase : this.#phrase)) this.cancel();
        else if (this.flow === "restore") this.#go("restoreOptions");
        else this.#showPhrase();
        return;
      default:
    }
  }

  /** Drops everything and returns to Welcome. */
  cancel() {
    this.#clear();
    this.flow = null;
    this.#go("welcome");
  }

  /** Drops every secret (the screen went away). */
  dispose() {
    this.#clear();
  }

  // Private

  async #generate() {
    try {
      this.#phrase = await this.api.generateMnemonic(this.wordCount);
    } catch (e) {
      this.failure = failureText(e);
      this.#go("failed");
      return;
    }
    this.#showPhrase();
  }

  #showPhrase() {
    if (!this.#phrase) return;
    this.phraseWords = wordsOf(this.#phrase);
    this.verifyChips = [];
    this.verifyChallenge = [];
    this.verifiedCount = 0;
    this.verifyMistake = false;
    this.#go("showPhrase");
  }

  async #afterPassphrase() {
    this.passphraseError = null;
    if (this.flow === "create") await this.#generate();
    else await this.finish();
  }

  async #readVault(): Promise<boolean> {
    try {
      this.vaultState = (await this.api.lockStatus()).state;
      return true;
    } catch (e) {
      this.failure = failureText(e);
      this.#go("failed");
      return false;
    }
  }

  #needsUnlock(next: "create" | "restore"): boolean {
    if (!this.vaultState || !NEEDS_UNLOCK.has(this.vaultState)) return false;
    this.#afterUnlock = next;
    this.unlockError = null;
    return true;
  }

  #shuffled<T>(items: T[]): T[] {
    const out = [...items];
    for (let i = out.length - 1; i > 0; i--) {
      const j = Math.floor(this.#random() * (i + 1));
      [out[i], out[j]] = [out[j], out[i]];
    }
    return out;
  }

  #go(step: OnboardingStep) {
    this.step = step;
    this.changed();
  }

  #clear() {
    this.#phrase = null;
    this.#bip39 = null;
    this.#vaultPassphrase = null;
    this.#skipEncryption = false;
    this.phraseWords = [];
    this.verifyChips = [];
    this.verifyChallenge = [];
    this.verifiedCount = 0;
    this.verifyMistake = false;
    this.entry.clear();
    this.failure = null;
    this.walletId = null;
  }
}
