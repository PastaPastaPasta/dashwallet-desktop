// Settings ▸ Security (UX-SPEC §4.13, QT-111, IOS-016, IOS-109): the Swift `SecurityViewModel`
// and dash-qt's `AskPassphraseDialog`, ported. Every check of the vault passphrase goes through
// `LockModel.attempt` (U-02 D12), as each counts a failure in the vault. Passphrases stay in the
// call that uses them; a model keeps none past its step.

import { hasCode, type Backend } from "../api/backend";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { LockStatus } from "../api/generated/LockStatus";
import { LockModel } from "../onboarding/lock";
import { failureText, Model } from "../onboarding/model";
import { passphraseProblem } from "../onboarding/passphrase";

export const SecurityText = {
  encrypted: "Encrypted",
  notEncrypted: "Not encrypted",
  encryptHint: "Anyone who can use this computer can spend from this wallet. Encrypt it with a passphrase.",
  /** dash-qt `AskPassphraseDialog` after `encryptWallet`. */
  walletEncrypted:
    "Your wallet is now encrypted. Remember that encrypting your wallet cannot fully protect your funds from being stolen by malware infecting your computer.",
  lockedAfterEncrypt: "The wallet is locked: unlock it with the new passphrase.",
  /** dash-qt `AskPassphraseDialog` after `changePassphrase`. */
  passphraseChanged: "Wallet passphrase was successfully changed.",
  sameAsOld: "The new passphrase is the same as the current one.",
  quickUnlockUnavailable: "Not available on this computer",
  quickUnlockWhy: "Touch ID comes with a signed macOS build (U-08), Windows Hello with the Windows release (W-02).",
  requireAuthentication: "Require authentication for every payment",
  requireAuthenticationHint: "Ask for the passphrase before each payment, even while the wallet is unlocked.",
  encryptFirst: "Encrypt the wallet first.",
  autohideBalance: "Hide balances when the app starts",
  lockedAutomatically: "Locked automatically.",
  autoLockSeconds: (secs: number) =>
    secs % 60 === 0 ? `${secs / 60} minute${secs === 60 ? "" : "s"}` : `${secs} second${secs === 1 ? "" : "s"}`,
  /** IOS-009 "Delete All Wallets?". */
  wipeTitle: "Delete all wallet data?",
  wipeMessage:
    "Every wallet on this network is removed from this computer: its keys, its recovery phrase, its history and its settings. Your coins stay on the network. A wallet that spends comes back from its recovery phrase, or from a backup or export you kept; a watch-only wallet comes back with its extended public key.",
  /** iOS's sentence, typed exactly. */
  wipeAcceptPhrase: "I accept that I will lose my coins if I no longer possess the recovery phrase",
  acceptPhraseMismatch: "Type the sentence exactly as shown.",
  wiped: "The wallets and the vault were deleted. Backups and exports were kept.",
  /** Backups and exports are files the user chose to keep: the wipe leaves them (D27). */
  wipeKeeps: (dir: string) =>
    `Backups and exports in ${dir} are not deleted. An export for Dash Core holds private keys in plain text: delete it yourself if you no longer need it.`,
  wipeNoList: (why: string) => `The list of every wallet could not be read: ${why} Nothing was deleted.`,
  /** The wallets are gone but not all of the vault: dw-vault deletes the vault file before its
   * older copies, so what is left on disk is not known (D33, D45). */
  vaultKept: (why: string) =>
    `Every wallet was removed, but the vault could not be fully deleted${why ? `: ${why}` : "."} Try again. If you close this instead, part of it may stay on this computer, and a wallet you add next may be kept under the old passphrase.`,
  wipeStopped: (name: string, why: string, removed: number) =>
    `${name} could not be removed: ${why} ${removed === 0 ? "Nothing was deleted." : `${removed === 1 ? "The wallet" : `The ${removed} wallets`} before it ${removed === 1 ? "was" : "were"} removed; the others and the vault were kept.`}`,
};

/** The auto-lock choices (IOS-015 without "Immediately", D12): seconds, `null` = Never. */
export const AUTO_LOCK_CHOICES: { secs: number | null; label: string }[] = [
  { secs: null, label: "Never" },
  { secs: 60, label: "1 minute" },
  { secs: 300, label: "5 minutes" },
  { secs: 3600, label: "1 hour" },
  { secs: 86_400, label: "24 hours" },
];

type LockApi = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly">;
const passphrase = (p: string): CredentialSpec => ({ kind: "passphrase", passphrase: p });

/** The vault's own credential for `pass`: its passphrase, or none on an unencrypted vault. */
export const vaultCredential = (encrypted: boolean, pass: string): CredentialSpec => (encrypted ? passphrase(pass) : { kind: "unencrypted" });

/** Encrypt wallet (QT-111, D9): the new passphrase twice, a `changeCredential` grant with the
 * unencrypted vault's own credential, then `encryptVault`. The vault ends locked. */
export class EncryptModel extends Model {
  step: "form" | "working" | "done" = "form";
  error: string | null = null;

  constructor(private readonly api: Pick<Backend, "authorize" | "encryptVault" | "revokeGrant">) {
    super();
  }

  edited() {
    this.error = null;
    this.changed();
  }

  async encrypt(text: string, confirmation: string) {
    if (this.step !== "form") return;
    const problem = passphraseProblem(text, confirmation);
    if (problem) return this.#fail(problem);
    this.error = null;
    this.step = "working";
    this.changed();
    let grant: string | null = null;
    try {
      grant = (await this.api.authorize({ kind: "changeCredential" }, null, { kind: "unencrypted" })).id;
      await this.api.encryptVault(text, grant);
      this.step = "done";
      this.changed();
    } catch (e) {
      if (grant) this.api.revokeGrant(grant).catch(() => {});
      this.step = "form";
      this.#fail(failureText(e));
    }
  }

  #fail(message: string) {
    this.error = message;
    this.changed();
  }
}

/** Change passphrase (QT-111, IOS-108, D10): the current passphrase and the new one twice. The new
 * one is checked first; `changePassphrase` then runs as one passphrase attempt. */
export class ChangePassphraseModel extends Model {
  readonly lock: LockModel;
  /** A problem with the new passphrase (the lock model words the current one's). */
  error: string | null = null;
  done = false;

  constructor(private readonly api: LockApi & Pick<Backend, "changePassphrase">) {
    super();
    this.lock = new LockModel(api);
  }

  load() {
    return this.lock.load();
  }

  get message(): string | null {
    return this.error ?? this.lock.message;
  }

  edited() {
    this.error = null;
    this.changed();
  }

  async change(current: string, text: string, confirmation: string) {
    if (this.done || this.lock.working) return;
    const problem = passphraseProblem(text, confirmation) ?? (text === current ? SecurityText.sameAsOld : null);
    this.error = problem;
    this.changed();
    if (problem) return;
    if (await this.lock.attempt(current, (old) => this.api.changePassphrase(old, text))) {
      this.done = true;
      this.changed();
    }
  }
}

/** Require authentication for every payment (IOS-016, D13): a change either way asks for the
 * passphrase and takes a `changeCredential` grant. */
export class PaymentAuthModel extends Model {
  on: boolean | null = null;
  /** The value the prompt is asking to set; `null` while no change is asked for. */
  pending: boolean | null = null;
  error: string | null = null;
  readonly lock: LockModel;

  constructor(private readonly api: LockApi & Pick<Backend, "paymentAuthentication" | "setPaymentAuthentication" | "authorize" | "revokeGrant">) {
    super();
    this.lock = new LockModel(api);
  }

  async load() {
    try {
      this.on = await this.api.paymentAuthentication();
    } catch (e) {
      this.error = failureText(e);
    }
    this.changed();
  }

  ask(on: boolean) {
    if (on === this.on || this.busy) return;
    this.#round++;
    this.pending = on;
    this.error = null;
    this.changed();
    void this.lock.load();
  }

  /** Closes the sheet. Refused while the passphrase is checked or the change written (D31). */
  cancel() {
    if (this.busy) return;
    this.#round++;
    this.pending = null;
    this.changed();
  }

  /** The card is gone: a passphrase still being checked changes nothing. */
  dispose() {
    this.#round++;
  }

  /** Writing the change, after the passphrase was accepted. */
  saving = false;
  /** Each question is a round; closing it ends the round, and a late grant is revoked. */
  #round = 0;

  get busy(): boolean {
    return this.lock.working || this.saving;
  }

  async confirm(p: string) {
    const on = this.pending;
    if (on === null || this.busy) return;
    const round = this.#round;
    let grant: string | null = null;
    const authorized = await this.lock.attempt(p, async (pass) => {
      grant = (await this.api.authorize({ kind: "changeCredential" }, null, passphrase(pass))).id;
    });
    if (!authorized || !grant) return;
    // The question was closed meanwhile: nothing changes (D31).
    if (round !== this.#round) return void this.api.revokeGrant(grant).catch(() => {});
    this.saving = true;
    this.changed();
    try {
      await this.api.setPaymentAuthentication(on, grant);
      this.on = on;
      this.pending = null;
    } catch (e) {
      this.api.revokeGrant(grant).catch(() => {});
      this.error = failureText(e);
    }
    this.saving = false;
    this.changed();
  }
}

export interface WipeWallet {
  walletId: string;
  name: string;
  loaded: boolean;
}

type WipeApi = LockApi &
  Pick<Backend, "walletLoadStates" | "loadWallet" | "authorize" | "removeWallet" | "destroyVault" | "revokeGrant">;

/** Wipe wallet (IOS-009, IOS-109, D16): the typed sentence, the passphrase, then every wallet
 * removed with its `wipe` grant and the vault destroyed. A failed removal stops before the vault
 * goes, and says which wallet. */
export class WipeModel extends Model {
  step: "sentence" | "passphrase" | "working" | "done" | "stopped" = "sentence";
  error: string | null = null;
  encrypted = true;
  readonly lock: LockModel;

  constructor(private readonly api: WipeApi) {
    super();
    this.lock = new LockModel(api);
  }

  async load() {
    try {
      this.encrypted = (await this.api.lockStatus()).encrypted;
    } catch (e) {
      this.error = failureText(e);
    }
    if (this.encrypted) await this.lock.load();
    this.changed();
  }

  /** The sentence must be typed exactly (whitespace at the ends aside). */
  accept(sentence: string) {
    if (this.step !== "sentence") return;
    if (sentence.trim() !== SecurityText.wipeAcceptPhrase) {
      this.error = SecurityText.acceptPhraseMismatch;
      return this.changed();
    }
    this.error = null;
    this.step = "passphrase";
    this.changed();
  }

  /** Runs the wipe; `p` is ignored on an unencrypted vault. The first grant checks the
   * passphrase under the lockout policy; the other wallets reuse it. */
  async wipe(p: string) {
    if (this.step !== "passphrase" || this.busy) return;
    this.#running = true;
    // The sheet shows it at once: Close and the button are off while the list is read (D41).
    this.changed();
    try {
      await this.#wipe(p);
    } finally {
      this.#running = false;
      this.changed();
    }
  }

  async #wipe(p: string) {
    this.error = null;
    const epoch = this.#epoch;
    let wallets: WipeWallet[];
    try {
      // Every registered wallet, closed ones too. `listWallets` holds only the open ones, so a
      // wipe built on it would delete some wallets and keep the rest (D32).
      wallets = (await this.api.walletLoadStates()).map((s) => ({ walletId: s.walletId, name: s.name, loaded: s.loaded }));
    } catch (e) {
      return this.#stop(SecurityText.wipeNoList(failureText(e)));
    }
    let first = null as string | null;
    // The engine's answer to the vault's deletion (D43), or why it failed (D44).
    let status = null as LockStatus | null;
    let kept = null as unknown;
    // No wallet left (an earlier wipe stopped after its last removal): only the vault goes. Only
    // a passphrase refusal stays on the passphrase step; any other failure stops the wipe (D44).
    const check = (pass: string) =>
      wallets.length > 0
        ? this.api.authorize({ kind: "wipe" }, wallets[0].walletId, vaultCredential(this.encrypted, pass)).then((g) => void (first = g.id))
        : this.api.destroyVault(vaultCredential(this.encrypted, pass)).then(
            (s) => void (status = s),
            (e: unknown) => {
              if (hasCode(e, "vault.wrong_passphrase") || hasCode(e, "vault.throttled")) throw e;
              // The lock then counts the passphrase as good, which the engine may not have
              // checked; harmless, as the step stops and Try again reads the count again.
              kept = e;
            },
          );
    if (this.encrypted) {
      if (!(await this.lock.attempt(p, check))) return this.changed();
    } else {
      this.step = "working";
      this.changed();
      const failed = await check("").then(
        () => null,
        (e: unknown) => e,
      );
      if (failed) return this.#stop(failureText(failed));
    }
    // The sheet went away while the passphrase was checked: nothing is deleted (D31).
    if (epoch !== this.#epoch) {
      if (first) this.api.revokeGrant(first).catch(() => {});
      return;
    }
    if (wallets.length > 0) {
      this.#go("working");
      // The engine removes only loaded wallets. Every closed one is opened before the first
      // removal, so a wallet that cannot open stops the wipe with nothing deleted (D26).
      for (const w of wallets.filter((w) => !w.loaded)) {
        try {
          await this.api.loadWallet(w.walletId);
        } catch (e) {
          if (first) this.api.revokeGrant(first).catch(() => {});
          return this.#stop(SecurityText.wipeStopped(w.name, failureText(e), 0));
        }
      }
      // From here the wipe runs to its end or its first failure, whoever watches.
      for (const [i, w] of wallets.entries()) {
        let grant: string | null = i === 0 ? first : null;
        try {
          grant ??= (await this.api.authorize({ kind: "wipe" }, w.walletId, vaultCredential(this.encrypted, p))).id;
          await this.api.removeWallet(w.walletId, grant);
        } catch (e) {
          if (grant) this.api.revokeGrant(grant).catch(() => {});
          return this.#stop(SecurityText.wipeStopped(w.name, failureText(e), i));
        }
      }
      try {
        status = await this.api.destroyVault(vaultCredential(this.encrypted, p));
      } catch (e) {
        kept = e;
      }
    }
    if (kept) return this.#stop(SecurityText.vaultKept(failureText(kept)));
    // Done only once the engine says there is no vault (D33), in its answer to the deletion: a
    // separate read that failed would report a deleted vault as kept (D43).
    if (status?.state !== "noVault") return this.#stop(SecurityText.vaultKept(""));
    this.#go("done");
  }

  /** After a stop: the passphrase again, then the wipe goes on with what is left. */
  retry() {
    if (this.step !== "stopped") return;
    this.error = null;
    this.#go("passphrase");
    if (this.encrypted) void this.lock.load();
  }

  // A wipe is under way, from its first engine call (a second click starts none, D41).
  #running = false;
  // Not a flag: StrictMode cancels and mounts the same model again (D37).
  #epoch = 0;

  /** A wipe is under way: the sheet stays open (D31, D41). */
  get busy(): boolean {
    return this.#running || this.lock.working || this.step === "working";
  }

  /** The sheet is gone: a wipe still checking the passphrase deletes nothing. */
  cancel() {
    this.#epoch++;
  }

  #go(step: WipeModel["step"]) {
    this.step = step;
    this.changed();
  }

  #stop(message: string) {
    // The stop ends the run before it renders: Close is offered with it (D39).
    this.#running = false;
    this.error = message;
    this.step = "stopped";
    this.changed();
  }
}
