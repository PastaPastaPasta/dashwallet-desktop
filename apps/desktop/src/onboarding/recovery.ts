// Show Recovery Phrase (IOS-003, IOS-108, U-02 D13): a single-use `revealSecret` grant, then the
// phrase while the sheet shows it. Closing the sheet drops it. The passphrase goes through the
// lock screen's policy (`LockModel.attempt`): `authorize` counts a failure as `unlock` does.

import type { Backend } from "../api/backend";
import { LockModel } from "./lock";
import { failureText, Model, type WalletChoice } from "./model";
import { wordsOf } from "./words";

export type RecoveryStep = "loading" | "wallet" | "confirm" | "working" | "shown" | "unavailable" | "failed";

export const RecoveryText = {
  title: "Recovery phrase",
  warning:
    "Anyone with your recovery phrase can take your Dash. Never share it, and make sure no one can see your screen.",
  writeDown: "Write down these words in order and keep them somewhere safe. Anyone with them can spend your funds.",
  noPhrase: "No wallet on this network has a recovery phrase in Dash Wallet.",
};

type Api = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "listWallets" | "authorize" | "revealMnemonic" | "revokeGrant">;

export class RecoveryPhraseModel extends Model {
  step: RecoveryStep = "loading";
  wallets: WalletChoice[] = [];
  walletId: string | null = null;
  /** A passphrase is needed (encrypted vault). */
  encrypted = true;
  /** The passphrase prompt's count, wait and copy. */
  readonly lock: LockModel;
  /** A failure outside the passphrase prompt (loading, an unencrypted vault's reveal). */
  error: string | null = null;
  /** The phrase while it shows; empty otherwise. */
  words: string[] = [];
  /** The wallet also has a BIP39 passphrase; the sheet says so but never shows it. */
  hasBip39 = false;
  /** Bumped when the sheet closes: a reveal started before is dropped (StrictMode closes and
   * opens the same model again, so not a flag). */
  #epoch = 0;

  constructor(private readonly api: Api) {
    super();
    this.lock = new LockModel(api);
  }

  /** `preferred` is chosen when it has a phrase (the window's wallet). */
  async load(preferred: string | null = null) {
    try {
      const [status, summaries] = await Promise.all([this.api.lockStatus(), this.api.listWallets()]);
      const secrets = new Set(status.walletsWithSecrets);
      this.encrypted = status.encrypted;
      this.wallets = summaries
        .filter((s) => s.hasMnemonic && !s.watchOnly && secrets.has(s.walletId))
        .map((s) => ({ walletId: s.walletId, name: s.name }));
    } catch (e) {
      this.error = failureText(e);
      return this.#go("failed");
    }
    if (this.encrypted) await this.lock.load();
    if (this.wallets.length === 0) this.#go("unavailable");
    else if (preferred && this.wallets.some((w) => w.walletId === preferred)) this.choose(preferred);
    else if (this.wallets.length === 1) this.choose(this.wallets[0].walletId);
    else this.#go("wallet");
  }

  choose(walletId: string) {
    this.walletId = walletId;
    this.error = null;
    this.#go("confirm");
  }

  /** Checks the passphrase (ignored on an unencrypted vault) and shows the phrase. */
  async reveal(passphrase: string) {
    const walletId = this.walletId;
    if (this.step !== "confirm" || !walletId || this.lock.working) return;
    const epoch = this.#epoch;
    this.error = null;
    let grant: string | null = null;
    const authorize = async (p: string) => {
      const credential = this.encrypted ? ({ kind: "passphrase", passphrase: p } as const) : ({ kind: "unencrypted" } as const);
      grant = (await this.api.authorize({ kind: "revealSecret" }, walletId, credential)).id;
    };
    let authorized: boolean;
    if (this.encrypted) authorized = await this.lock.attempt(passphrase, authorize);
    else {
      // Out of `confirm` at once: the button takes no second click.
      this.#go("working");
      authorized = await authorize("").then(
        () => true,
        (e) => {
          this.error = failureText(e);
          return false;
        },
      );
      if (!authorized) this.#go("confirm");
    }
    if (!authorized || !grant) return this.changed();
    if (epoch !== this.#epoch) return this.#revoke(grant);
    this.#go("working");
    try {
      const revealed = await this.api.revealMnemonic(walletId, grant);
      // The sheet closed meanwhile: the phrase is not kept.
      if (epoch !== this.#epoch) return;
      this.words = wordsOf(revealed.phrase);
      this.hasBip39 = revealed.bip39Passphrase !== "";
      this.#go("shown");
    } catch (e) {
      // An issued grant that did not reach the reveal is revoked, not left to expire.
      this.#revoke(grant);
      this.error = failureText(e);
      this.#go("confirm");
    }
  }

  /** Hides the phrase (the sheet closed); a reveal still in flight is dropped. */
  dispose() {
    this.#epoch++;
    this.words = [];
    this.hasBip39 = false;
  }

  #revoke(grant: string) {
    this.api.revokeGrant(grant).catch(() => {});
  }

  #go(step: RecoveryStep) {
    this.step = step;
    this.changed();
  }
}
