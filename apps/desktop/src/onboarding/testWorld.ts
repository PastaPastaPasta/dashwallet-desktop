// A fake engine for the U-02 view-model tests (the Swift `FakeWorld`, ported): records calls and
// answers as dw-vault and the engine would. Imported by tests only.

import type { Backend } from "../api/backend";
import type { AppError } from "../api/generated/AppError";
import type { ImportOptions } from "../api/generated/ImportOptions";
import type { LockStatus } from "../api/generated/LockStatus";
import type { MnemonicCheck } from "../api/generated/MnemonicCheck";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { BIP39_ENGLISH } from "./bip39English";
import { normalizePhrase, RESTORE_COUNTS, wordsOf } from "./words";

export const PHRASE = "abandon ability able about above absent absorb abstract absurd abuse access accident";
export const WALLET_A = "a1".repeat(32);
export const WALLET_B = "b2".repeat(32);
export const NEXT_WALLET = "c3".repeat(32);

export const appError = (code: AppError["code"], data: AppError["data"] = null, message: string = code): AppError => ({ code, message, data });

/** A deterministic [0, 1) generator (the Swift tests' LCG). */
export function seeded(seed: bigint): () => number {
  let state = seed;
  return () => {
    state = (state * 6_364_136_223_846_793_005n + 1_442_695_040_888_963_407n) & 0xffff_ffff_ffff_ffffn;
    return Number(state >> 11n) / 2 ** 53;
  };
}

export const status = (patch: Partial<LockStatus> = {}): LockStatus => ({
  state: "unlocked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: [],
  ...patch,
});

export const summary = (walletId: string, name: string, patch: Partial<WalletSummary> = {}): WalletSummary => ({
  walletId,
  name,
  watchOnly: false,
  hasMnemonic: true,
  hd: true,
  birthHeight: 0,
  createdAt: null,
  balances: null,
  ...patch,
});

/** dw-vault `throttle_wait_secs`. */
export const throttleSecs = (failed: number) => (failed < 3 ? 0 : 6 ** (failed - 3) * 60);

type Api = Pick<
  Backend,
  | "lockStatus"
  | "listWallets"
  | "unlock"
  | "unlockMixingOnly"
  | "createVault"
  | "generateMnemonic"
  | "checkMnemonic"
  | "importWallet"
  | "authorize"
  | "revokeGrant"
  | "revealMnemonic"
  | "recoverWithMnemonic"
>;

export class FakeWorld implements Api {
  status: LockStatus = status({ state: "noVault", encrypted: false });
  passphrase = "right";
  /** The fake clock, in ms. */
  clock = 1_700_000_000_000;
  lastFailureAt: number | null = null;
  wallets: WalletSummary[] = [];
  /** Phrases by wallet: what `recoverWithMnemonic` and `revealMnemonic` check and return. */
  phrases = new Map<string, string>();
  checks = new Map<string, MnemonicCheck>();
  generated = PHRASE;
  /** `checkMnemonic` of these phrases answers once the promise settles (a late reply). */
  held = new Map<string, Promise<void>>();

  generateCalls: number[] = [];
  creates: (string | null)[] = [];
  imports: { mnemonic: string; bip39Passphrase: string | null; options: ImportOptions }[] = [];
  importErrors: AppError[] = [];
  unlockCalls: { passphrase: string; mixingOnly: boolean }[] = [];
  unlockErrors: AppError[] = [];
  recoveries: { walletId: string; mnemonic: string; bip39Passphrase: string; newPassphrase: string }[] = [];
  authorizations: { kind: string; walletId: string | null; credential: string }[] = [];
  revoked: string[] = [];
  revealErrors: AppError[] = [];

  now = () => this.clock;

  lockStatus = async () => {
    const wait = this.lastFailureAt === null ? 0 : this.lastFailureAt + throttleSecs(this.status.failedAttempts) * 1000 - this.clock;
    return { ...this.status, retryAfterSecs: wait > 0 ? Math.ceil(wait / 1000) : null };
  };

  listWallets = async () => this.wallets;

  #unlock(passphrase: string, mixingOnly: boolean): LockStatus {
    this.unlockCalls.push({ passphrase, mixingOnly });
    const scripted = this.unlockErrors.shift();
    if (scripted) throw scripted;
    if (passphrase !== this.passphrase) {
      const failedAttempts = this.status.failedAttempts + 1;
      this.status = { ...this.status, failedAttempts };
      this.lastFailureAt = this.clock;
      const wait = throttleSecs(failedAttempts);
      throw appError("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts, retryAfterSecs: wait || null });
    }
    this.status = { ...this.status, state: mixingOnly ? "unlockedMixingOnly" : "unlocked", failedAttempts: 0 };
    this.lastFailureAt = null;
    return this.status;
  }

  unlock = async (passphrase: string) => this.#unlock(passphrase, false);
  unlockMixingOnly = async (passphrase: string) => this.#unlock(passphrase, true);

  createVault = async (passphrase: string | null) => {
    this.creates.push(passphrase);
    this.status = status({ state: passphrase === null ? "noKeys" : "unlocked", encrypted: passphrase !== null });
    return this.status;
  };

  generateMnemonic = async (wordCount: number) => {
    this.generateCalls.push(wordCount);
    return this.generated;
  };

  checkMnemonic = async (phrase: string): Promise<MnemonicCheck> => {
    await this.held.get(phrase);
    const scripted = this.checks.get(phrase);
    if (scripted) return scripted;
    const words = wordsOf(normalizePhrase(phrase));
    const unknownWordIndices = words.flatMap((w, i) => (BIP39_ENGLISH.includes(w) ? [] : [i]));
    const known = unknownWordIndices.length === 0;
    return {
      wordCount: words.length,
      unknownWordIndices,
      language: known ? "english" : null,
      checksum: known && RESTORE_COUNTS.has(words.length) ? "valid" : "invalid",
    };
  };

  importWallet = async (mnemonic: string, bip39Passphrase: string | null, options: ImportOptions) => {
    this.imports.push({ mnemonic, bip39Passphrase, options });
    const scripted = this.importErrors.shift();
    if (scripted) throw scripted;
    if (this.status.state === "locked" || this.status.state === "noVault") throw appError(this.status.state === "locked" ? "vault.locked" : "vault.no_vault");
    this.status = { ...this.status, walletsWithSecrets: [...this.status.walletsWithSecrets, NEXT_WALLET].sort() };
    return NEXT_WALLET;
  };

  authorize: Api["authorize"] = async (purpose, walletId, credential) => {
    this.authorizations.push({ kind: purpose.kind, walletId, credential: credential.kind });
    if (credential.kind === "passphrase" && credential.passphrase !== this.passphrase)
      throw appError("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts: 1, retryAfterSecs: null });
    return { id: `grant-${this.authorizations.length}`, purpose, walletId, expiresAt: 0, singleUse: true };
  };

  revokeGrant = async (grantId: string) => {
    this.revoked.push(grantId);
    return null;
  };

  revealMnemonic = async (walletId: string) => {
    const scripted = this.revealErrors.shift();
    if (scripted) throw scripted;
    const phrase = this.phrases.get(walletId);
    if (!phrase) throw appError("vault.no_secret");
    return { phrase, bip39Passphrase: "" };
  };

  recoverWithMnemonic = async (walletId: string, mnemonic: string, bip39Passphrase: string, newPassphrase: string) => {
    this.recoveries.push({ walletId, mnemonic, bip39Passphrase, newPassphrase });
    if (this.phrases.get(walletId) !== mnemonic) throw appError("vault.recovery_mismatch");
    const walletsWithoutSecrets = this.status.walletsWithSecrets.filter((w) => w !== walletId);
    this.status = status({ walletsWithSecrets: [walletId] });
    this.passphrase = newPassphrase;
    return { status: this.status, walletsWithoutSecrets };
  };
}

/** Lets pending promise callbacks run. */
export const settle = () => new Promise((r) => setTimeout(r, 0));
