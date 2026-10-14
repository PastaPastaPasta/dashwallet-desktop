// Settings ▸ Security (UX-SPEC §4.13, QT-111, IOS-016, IOS-109): the Swift `SecurityViewModel`
// and dash-qt's `AskPassphraseDialog`, ported. Every check of the vault passphrase goes through
// `LockModel.attempt` (U-02 D12), as each counts a failure in the vault. Passphrases stay in the
// call that uses them; a model keeps none past its step.

import type { Backend } from "../api/backend";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
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
  /** IOS-009 "Delete All Wallets?". */
  wipeTitle: "Delete all wallet data?",
  wipeMessage:
    "Every wallet on this network is removed from this computer: its keys, its recovery phrase, its history and its settings. Your coins stay on the network: only your recovery phrase can bring them back.",
  /** iOS's sentence, typed exactly. */
  wipeAcceptPhrase: "I accept that I will lose my coins if I no longer possess the recovery phrase",
  acceptPhraseMismatch: "Type the sentence exactly as shown.",
  wiped: "All wallet data was deleted.",
  wipeStopped: (name: string, why: string) => `${name} could not be removed: ${why} Nothing else was deleted.`,
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
    if (on === this.on) return;
    this.pending = on;
    this.error = null;
    this.changed();
    void this.lock.load();
  }

  cancel() {
    this.pending = null;
    this.changed();
  }

  async confirm(p: string) {
    const on = this.pending;
    if (on === null) return;
    let grant: string | null = null;
    const authorized = await this.lock.attempt(p, async (pass) => {
      grant = (await this.api.authorize({ kind: "changeCredential" }, null, passphrase(pass))).id;
    });
    if (!authorized || !grant) return;
    try {
      await this.api.setPaymentAuthentication(on, grant);
      this.on = on;
      this.pending = null;
    } catch (e) {
      this.api.revokeGrant(grant).catch(() => {});
      this.error = failureText(e);
    }
    this.changed();
  }
}

export interface WipeWallet {
  walletId: string;
  name: string;
  loaded: boolean;
}

type WipeApi = LockApi &
  Pick<Backend, "walletLoadStates" | "listWallets" | "loadWallet" | "authorize" | "removeWallet" | "destroyVault" | "revokeGrant">;

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
    if (this.step !== "passphrase" || this.lock.working) return;
    const credential = (pass: string): CredentialSpec => (this.encrypted ? passphrase(pass) : { kind: "unencrypted" });
    let wallets: WipeWallet[];
    try {
      wallets = await this.#wallets();
    } catch (e) {
      return this.#stop(failureText(e));
    }
    let first = null as string | null;
    // No wallet left (an earlier wipe stopped after its last removal): only the vault goes.
    const check = (pass: string) =>
      wallets.length > 0
        ? this.api.authorize({ kind: "wipe" }, wallets[0].walletId, credential(pass)).then((g) => void (first = g.id))
        : this.api.destroyVault(credential(pass));
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
    if (wallets.length === 0) return this.#go("done");
    this.#go("working");
    for (const [i, w] of wallets.entries()) {
      let grant: string | null = i === 0 ? first : null;
      try {
        grant ??= (await this.api.authorize({ kind: "wipe" }, w.walletId, credential(p))).id;
        // The engine removes only loaded wallets: a closed one is opened first.
        if (!w.loaded) await this.api.loadWallet(w.walletId);
        await this.api.removeWallet(w.walletId, grant);
      } catch (e) {
        if (grant) this.api.revokeGrant(grant).catch(() => {});
        return this.#stop(SecurityText.wipeStopped(w.name, failureText(e)));
      }
    }
    try {
      await this.api.destroyVault(credential(p));
    } catch (e) {
      return this.#stop(failureText(e));
    }
    this.#go("done");
  }

  /** Every registered wallet, closed ones too; the open ones when load states are unknown. */
  async #wallets(): Promise<WipeWallet[]> {
    const states = await this.api.walletLoadStates().catch(() => null);
    if (states) return states.map((s) => ({ walletId: s.walletId, name: s.name, loaded: s.loaded }));
    return (await this.api.listWallets()).map((s) => ({ walletId: s.walletId, name: s.name, loaded: true }));
  }

  #go(step: WipeModel["step"]) {
    this.step = step;
    this.changed();
  }

  #stop(message: string) {
    this.error = message;
    this.step = "stopped";
    this.changed();
  }
}
