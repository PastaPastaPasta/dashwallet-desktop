// Create and restore (IOS-002, IOS-004, IOS-007, IOS-010, QT-102…105, QT-111): the Swift
// `OnboardingViewModel`, ported, in U-02's order (D2): create asks for the vault passphrase
// before the phrase and adds the wallet at the last correct chip; restore asks for it last.
// Nothing is stored before the wallet is added, and the phrase is dropped once it is.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { LockStateName } from "../api/generated/LockStateName";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { birthHeightFor } from "./birthHeight";
import { LockModel } from "./lock";
import { failureText, Model } from "./model";
import { passphraseProblem } from "./passphrase";
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

/** Where a restore's scan starts: genesis, a creation date's estimate, or a block height. */
export type ScanStart = "genesis" | "date" | "height";

export interface RestoreOptions {
  /** `null`: the engine's default, its tip, right for a new wallet only. */
  birthHeight: number | null;
  coreCompat: boolean;
}

export const OnboardingText = {
  birthDateInFuture: "The wallet creation date cannot be in the future.",
};

type Api = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "createVault" | "generateMnemonic" | "checkMnemonic" | "importWallet">;

/** A number in [0, 1) from the system's CSPRNG. */
const systemRandom = () => crypto.getRandomValues(new Uint32Array(1))[0] / 2 ** 32;

/** Vault states that need the passphrase before a wallet can be added. */
const NEEDS_UNLOCK: ReadonlySet<LockStateName> = new Set(["locked", "unlockedMixingOnly"]);

export class OnboardingModel extends Model {
  step: OnboardingStep = "welcome";
  flow: OnboardingFlow | null = null;
  vaultState: LockStateName | null = null;
  /** An engine call is running: the screen's actions wait, and a second submit is ignored. */
  busy = false;

  // Create.
  wordCount: CreateCount = 12;
  /** Display only: filled while the phrase step shows, cleared when it ends. */
  phraseWords: string[] = [];
  /** Phrase positions (0-based) to confirm, ascending. */
  verifyChallenge: number[] = [];
  verifyChips: VerifyChip[] = [];
  /** The words picked so far, in the challenge's order. */
  verifiedWords: string[] = [];
  verifyMistake = false;
  /** Wrong chips so far: each one is announced again. */
  verifyMistakes = 0;

  // The vault passphrase.
  passphraseError: string | null = null;
  /** The unlock step's count, wait and copy: the lock screen's policy (D12). */
  readonly lock: LockModel;

  // Restore.
  readonly entry: PhraseEntry;
  /** The scan start and the date (`YYYY-MM-DD`) or height typed for it, kept while the flow
   * goes back and forth (review r1 #8). */
  scanStart: ScanStart = "genesis";
  scanDate = "";
  scanHeight = "";
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
  /** Bumped when the screen goes away: a reply to a call made before is dropped. Not a flag:
   * StrictMode disposes and mounts the same model again. */
  #epoch = 0;
  /** `#epoch` when the running call started (calls run one at a time: `#exclusive`). */
  #callEpoch = 0;
  readonly #random: () => number;
  readonly #now: () => Date;

  constructor(
    private readonly api: Api,
    private readonly network: NetworkInfo["kind"],
    opts: { random?: () => number; now?: () => Date } = {},
  ) {
    super();
    this.entry = new PhraseEntry(api.checkMnemonic);
    this.lock = new LockModel(api);
    this.#random = opts.random ?? systemRandom;
    this.#now = opts.now ?? (() => new Date());
  }

  get verifiedCount(): number {
    return this.verifiedWords.length;
  }

  get isVerified(): boolean {
    return this.verifyChallenge.length > 0 && this.verifiedCount === this.verifyChallenge.length;
  }

  setWordCount(count: number) {
    if (!CREATE_COUNTS.includes(count as CreateCount)) return;
    this.wordCount = count as CreateCount;
    this.changed();
  }

  // Create

  /** Starts a new wallet: the passphrase or the unlock first when the vault needs one, then a
   * fresh phrase. Nothing is stored yet. */
  startCreate(): Promise<void> {
    return this.#exclusive(async () => {
      this.#clear();
      this.flow = "create";
      // A new wallet has no history: `null` lets the engine start at its tip (review H-5).
      this.options = { birthHeight: null, coreCompat: false };
      if (!(await this.#readVault())) return;
      if (this.vaultState === "noVault") this.#go("choosePassphrase");
      else if (this.#needsUnlock()) await this.#askUnlock("create");
      else await this.#generate();
    });
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
    this.verifiedWords = [];
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
    if (this.step !== "verifyPhrase" || this.isVerified || this.busy) return;
    const chip = this.verifyChips.find((c) => c.id === chipId && !c.used);
    if (!chip) return;
    if (this.verify(chip.word, this.verifyChallenge[this.verifiedCount])) {
      this.verifyChips = this.verifyChips.map((c) => (c === chip ? { ...c, used: true } : c));
      this.verifiedWords = [...this.verifiedWords, chip.word];
      this.verifyMistake = false;
      this.changed();
      if (this.isVerified) await this.#exclusive(() => this.#finish());
    } else {
      this.verifyMistake = true;
      this.verifyMistakes++;
      this.changed();
    }
  }

  // The vault passphrase

  /** The passphrase fields changed: the last refusal no longer applies. */
  passphraseEdited() {
    this.passphraseError = null;
    this.changed();
  }

  /** Takes the new vault's passphrase; `false` with `passphraseError` when it is refused. */
  async submitPassphrase(text: string, confirmation: string): Promise<boolean> {
    if (this.step !== "choosePassphrase" || this.busy) return false;
    const problem = passphraseProblem(text, confirmation);
    if (problem) {
      this.passphraseError = problem;
      this.changed();
      return false;
    }
    this.#vaultPassphrase = text;
    this.#skipEncryption = false;
    await this.#exclusive(() => this.#afterPassphrase());
    return true;
  }

  /** No passphrase: the vault's key goes to the OS secret store (D17). */
  async skipEncryption() {
    if (this.step !== "choosePassphrase" || this.busy) return;
    this.#vaultPassphrase = null;
    this.#skipEncryption = true;
    await this.#exclusive(() => this.#afterPassphrase());
  }

  /** Unlocks the existing vault fully under the lock screen's policy, then goes on. A wrong
   * passphrase stays here, with `lock.message`. */
  async unlockVault(passphrase: string) {
    if (this.step !== "unlockVault") return;
    await this.#exclusive(async () => {
      if (!(await this.lock.unlock(passphrase, false))) return;
      this.vaultState = this.lock.lockState;
      if (this.#afterUnlock === "create") await this.#generate();
      else if (this.#afterUnlock === "restore") this.#go("restorePhrase");
      else await this.#finish();
    });
  }

  // Restore

  startRestore(): Promise<void> {
    return this.#exclusive(async () => {
      this.#clear();
      this.flow = "restore";
      // Scan from genesis by default (review H-5); no lookahead: the engine's default.
      this.options = { birthHeight: 0, coreCompat: false };
      this.scanStart = "genesis";
      this.scanDate = "";
      this.scanHeight = "";
      this.birthDateError = null;
      if (!(await this.#readVault())) return;
      if (this.#needsUnlock()) await this.#askUnlock("restore");
      else this.#go("restorePhrase");
    });
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
  get bip39Passphrase(): string {
    return this.#bip39 ?? "";
  }

  setBip39Passphrase(text: string) {
    this.#bip39 = text || null;
    this.changed();
  }

  /** Dash Core compatibility (QT-104); stays on for a phrase that passes only Core's checksum. */
  setCoreCompatible(on: boolean) {
    this.options = { ...this.options, coreCompat: on || this.entry.coreOnly };
    this.changed();
  }

  setScanStart(start: ScanStart) {
    this.scanStart = start;
    if (start === "genesis") this.setBirthHeight(null);
    else if (start === "date") this.setScanDate(this.scanDate);
    else this.setScanHeight(this.scanHeight);
  }

  /** The date field's text (`YYYY-MM-DD`), read as a local date, so today is never in the
   * future (an input's `valueAsDate` is UTC midnight). */
  setScanDate(text: string) {
    this.scanDate = text;
    const ymd = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
    this.setBirthDate(ymd ? new Date(Number(ymd[1]), Number(ymd[2]) - 1, Number(ymd[3])) : null);
  }

  /** The height field's text; anything but a number scans from genesis. */
  setScanHeight(text: string) {
    this.scanHeight = text;
    const height = text.trim() === "" ? Number.NaN : Number(text);
    this.setBirthHeight(Number.isFinite(height) ? Math.max(0, Math.floor(height)) : null);
  }

  /** Scan start from the wallet's creation date; `null` scans from genesis. */
  setBirthDate(date: Date | null) {
    this.birthDateError = null;
    if (!date || Number.isNaN(date.getTime())) this.options = { ...this.options, birthHeight: 0 };
    else if (date > this.#now()) this.birthDateError = OnboardingText.birthDateInFuture;
    else this.options = { ...this.options, birthHeight: birthHeightFor(date, this.network) };
    this.changed();
  }

  /** Scan start as a block height; `null` (or anything but a whole number) scans from genesis:
   * NaN would cross IPC as `null`, which starts at the tip. */
  setBirthHeight(height: number | null) {
    this.birthDateError = null;
    const valid = height !== null && Number.isSafeInteger(height) && height >= 0;
    this.options = { ...this.options, birthHeight: valid ? height : 0 };
    this.changed();
  }

  async finishRestore() {
    if (this.step !== "restoreOptions" || this.birthDateError !== null) return;
    if (this.vaultState === "noVault") this.#go("choosePassphrase");
    else await this.#exclusive(() => this.#finish());
  }

  /** Returns to the previous screen; from a failure, to where the user can try again. Not while
   * an engine call runs: its reply would land in a flow the user left (review r2 #1). */
  back() {
    if (this.busy) return;
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
        if (this.#afterUnlock === "finish" && this.flow === "restore") this.#go("restoreOptions");
        else if (this.#afterUnlock === "finish") this.#showPhrase();
        else this.cancel();
        return;
      case "failed":
        if (!this.#activePhrase) this.cancel();
        else if (this.flow === "restore") this.#go("restoreOptions");
        else this.#showPhrase();
        return;
      default:
    }
  }

  /** Drops everything and returns to Welcome; not while an engine call runs. */
  cancel() {
    if (this.busy) return;
    this.#clear();
    this.flow = null;
    this.#go("welcome");
  }

  /** Drops every secret (the screen went away); replies still in flight are dropped too. */
  dispose() {
    this.#epoch++;
    this.#clear();
  }

  // Private

  /** Runs `op` unless another engine call is running (a double click, a second Enter). */
  async #exclusive(op: () => Promise<void>) {
    if (this.busy) return;
    this.busy = true;
    this.#callEpoch = this.#epoch;
    this.changed();
    try {
      await op();
    } finally {
      this.busy = false;
      this.changed();
    }
  }

  /** Creates the vault when there is none, adds the wallet, and drops every secret. A refused
   * vault passphrase (or a missing OS store) returns to Choose passphrase with the phrase and its
   * verification kept, so another passphrase can be tried (review r1 #6). A vault is created
   * without a passphrase only when the user chose that. */
  async #finish() {
    const phrase = this.#activePhrase;
    if (!phrase || this.step === "working") return;
    if (this.vaultState === "noVault" && !this.#vaultPassphrase && !this.#skipEncryption) return this.#go("choosePassphrase");
    this.#go("working");
    if (this.vaultState === "noVault") {
      try {
        this.vaultState = (await this.api.createVault(this.#skipEncryption ? null : this.#vaultPassphrase)).state;
      } catch (e) {
        this.passphraseError = failureText(e);
        this.#go("choosePassphrase");
        return;
      } finally {
        this.#vaultPassphrase = null;
        this.#skipEncryption = false;
      }
    }
    if (this.#stale) return;
    try {
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
        await this.#askUnlock("finish");
      } else {
        this.failure = failureText(e);
        this.#go("failed");
      }
    }
  }

  /** The screen went away since the running call started: its replies are dropped. */
  get #stale(): boolean {
    return this.#callEpoch !== this.#epoch;
  }

  get #activePhrase(): string | null {
    return this.flow === "restore" ? this.entry.phrase : this.#phrase;
  }

  async #generate() {
    try {
      const phrase = await this.api.generateMnemonic(this.wordCount);
      if (this.#stale) return;
      this.#phrase = phrase;
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
    this.#resetVerify();
    this.#go("showPhrase");
  }

  async #afterPassphrase() {
    this.passphraseError = null;
    // A create flow comes back here verified when the vault refused its passphrase.
    if (this.flow === "create" && !this.isVerified) await this.#generate();
    else await this.#finish();
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

  #needsUnlock(): boolean {
    return this.vaultState !== null && NEEDS_UNLOCK.has(this.vaultState);
  }

  /** The unlock step, with the vault's count and wait read first (D3, D12). */
  async #askUnlock(next: "create" | "restore" | "finish") {
    this.#afterUnlock = next;
    this.lock.message = null;
    this.#go("unlockVault");
    await this.lock.load();
  }

  #resetVerify() {
    this.verifyChips = [];
    this.verifyChallenge = [];
    this.verifiedWords = [];
    this.verifyMistake = false;
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
    this.#resetVerify();
    this.entry.clear();
    this.failure = null;
    this.walletId = null;
  }
}
