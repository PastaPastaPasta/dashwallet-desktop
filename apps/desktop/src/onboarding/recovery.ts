// Show Recovery Phrase (IOS-003, IOS-108, U-02 D13): a single-use `revealSecret` grant, then the
// phrase while the sheet shows it. Closing the sheet drops it.

import type { Backend } from "../api/backend";
import { Model, PASSPHRASE_REQUIRED, unlockFailureText } from "./model";
import { wordsOf } from "./words";
import type { WalletChoice } from "./forgot";

export type RecoveryStep = "loading" | "wallet" | "confirm" | "working" | "shown" | "unavailable" | "failed";

export const RecoveryText = {
  title: "Recovery phrase",
  warning:
    "Anyone with your recovery phrase can take your Dash. Never share it, and make sure no one can see your screen.",
  writeDown: "Write down these words in order and keep them somewhere safe. Anyone with them can spend your funds.",
  noPhrase: "No wallet on this network has a recovery phrase in Dash Wallet.",
};

type Api = Pick<Backend, "lockStatus" | "listWallets" | "authorize" | "revealMnemonic" | "revokeGrant">;

export class RecoveryPhraseModel extends Model {
  step: RecoveryStep = "loading";
  wallets: WalletChoice[] = [];
  walletId: string | null = null;
  /** A passphrase is needed (encrypted vault). */
  encrypted = true;
  error: string | null = null;
  /** The phrase while it shows; empty otherwise. */
  words: string[] = [];
  bip39Passphrase = "";

  constructor(private readonly api: Api) {
    super();
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
      this.error = unlockFailureText(e);
      return this.#go("failed");
    }
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
    if (this.step !== "confirm" || !walletId) return;
    if (this.encrypted && !passphrase) {
      this.error = PASSPHRASE_REQUIRED;
      return this.changed();
    }
    this.error = null;
    this.#go("working");
    let grant: string | null = null;
    try {
      grant = (
        await this.api.authorize(
          { kind: "revealSecret" },
          walletId,
          this.encrypted ? { kind: "passphrase", passphrase } : { kind: "unencrypted" },
        )
      ).id;
      const revealed = await this.api.revealMnemonic(walletId, grant);
      grant = null;
      this.words = wordsOf(revealed.phrase);
      this.bip39Passphrase = revealed.bip39Passphrase;
      this.#go("shown");
    } catch (e) {
      // An issued grant that did not reach the reveal is revoked, not left to expire.
      if (grant) this.api.revokeGrant(grant).catch(() => {});
      this.error = unlockFailureText(e);
      this.#go("confirm");
    }
  }

  /** Hides the phrase (the sheet closed). */
  dispose() {
    this.words = [];
    this.bip39Passphrase = "";
  }

  #go(step: RecoveryStep) {
    this.step = step;
    this.changed();
  }
}
