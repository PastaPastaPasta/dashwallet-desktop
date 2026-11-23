// Forgot passphrase (IOS-014, U-02 D11): the Swift `SecurityViewModel` flow, ported. A recovery
// phrase that derives one of the vault's wallets replaces the vault with one encrypted by a new
// passphrase; every other wallet with secrets becomes watch-only, which the user is told first.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import { t } from "../i18n";
import { failureText, listed, Model, type WalletChoice } from "./model";
import { passphraseProblem } from "./passphrase";
import { PhraseEntry } from "./phraseEntry";

export type ForgotStep =
  /** Nothing to recover: the vault holds no wallet's secrets. */
  | "unavailable"
  | "loading"
  /** Which wallet's phrase the user has (several wallets with secrets). */
  | "wallet"
  /** The other wallets lose their keys. */
  | "consequences"
  | "phrase"
  | "newPassphrase"
  | "working"
  | "done"
  | "failed";

export const ForgotText = {
  get prompt() {
    return t("onboarding.forgot.prompt");
  },
  get reset() {
    return t("onboarding.forgot.reset");
  },
  consequences: (names: string[]) =>
    names.length === 1 ? t("onboarding.forgot.consequencesOne", { names: listed(names) }) : t("onboarding.forgot.consequencesSeveral", { names: listed(names) }),
  walletsWithoutSecrets: (names: string[]) => t("onboarding.forgot.walletsWithoutSecrets", { names: listed(names) }),
};

/** `listWallets`: the wallets' ids and names, all a locked window may read of them. */
type Api = Pick<Backend, "lockStatus" | "checkMnemonic" | "recoverWithMnemonic"> & { listWallets(): Promise<{ walletId: string; name: string }[]> };

/** A wallet's name, or its id's first digits when the window has no summary for it. */
const nameOf = (walletId: string, names: Map<string, string>) => names.get(walletId) ?? t("onboarding.forgot.unnamedWallet", { id: walletId.slice(0, 8) });

export class ForgotPassphraseModel extends Model {
  step: ForgotStep = "loading";
  wallets: WalletChoice[] = [];
  walletId: string | null = null;
  readonly entry: PhraseEntry;
  error: string | null = null;
  /** Names of the wallets left watch-only by the recovery. */
  walletsWithoutSecrets: string[] = [];
  #bip39 = "";
  #names = new Map<string, string>();

  constructor(private readonly api: Api) {
    super();
    this.entry = new PhraseEntry(api.checkMnemonic);
  }

  /** The other wallets with secrets: they become watch-only. */
  get others(): string[] {
    return this.wallets.filter((w) => w.walletId !== this.walletId).map((w) => w.name);
  }

  async load() {
    try {
      const [status, summaries] = await Promise.all([this.api.lockStatus(), this.api.listWallets()]);
      this.#names = new Map(summaries.map((s) => [s.walletId, s.name]));
      const order = summaries.map((s) => s.walletId);
      const rank = (id: string) => (order.includes(id) ? order.indexOf(id) : order.length);
      this.wallets = status.walletsWithSecrets
        .toSorted((a, b) => rank(a) - rank(b))
        .map((walletId) => ({ walletId, name: nameOf(walletId, this.#names) }));
    } catch (e) {
      return this.#fail(failureText(e));
    }
    if (this.wallets.length === 0) this.#go("unavailable");
    else if (this.wallets.length === 1) this.chooseWallet(this.wallets[0].walletId);
    else this.#go("wallet");
  }

  chooseWallet(walletId: string) {
    if (!this.wallets.some((w) => w.walletId === walletId)) return;
    this.walletId = walletId;
    this.#go(this.others.length > 0 ? "consequences" : "phrase");
  }

  acceptConsequences() {
    if (this.step === "consequences") this.#go("phrase");
  }

  async updatePhrase(text: string) {
    this.error = null;
    await this.entry.update(text, () => this.changed());
  }

  setBip39Passphrase(text: string) {
    this.#bip39 = text;
  }

  continuePhrase() {
    if (this.step !== "phrase" || !this.entry.valid) return;
    this.#go("newPassphrase");
  }

  /** The passphrase fields changed: the last refusal no longer applies. */
  passphraseEdited() {
    this.error = null;
    this.changed();
  }

  /** Replaces the vault with one encrypted by `passphrase`. The phrase is dropped whatever the
   * outcome; a mismatch returns to the phrase step. */
  async submitNewPassphrase(passphrase: string, confirmation: string) {
    const phrase = this.entry.phrase;
    if (this.step !== "newPassphrase" || !phrase || !this.walletId) return;
    const problem = passphraseProblem(passphrase, confirmation);
    if (problem) {
      this.error = problem;
      this.changed();
      return;
    }
    this.error = null;
    this.#go("working");
    try {
      const result = await this.api.recoverWithMnemonic(this.walletId, phrase, this.#bip39, passphrase);
      this.#dropSecrets();
      this.walletsWithoutSecrets = result.walletsWithoutSecrets.map((id) => nameOf(id, this.#names));
      this.#go("done");
    } catch (e) {
      this.#dropSecrets();
      if (hasCode(e, "vault.recovery_mismatch")) {
        this.error = failureText(e);
        this.#go("phrase");
      } else this.#fail(failureText(e));
    }
  }

  back() {
    this.error = null;
    switch (this.step) {
      case "consequences":
        this.#go("wallet");
        return;
      case "phrase":
        this.#dropSecrets();
        if (this.others.length > 0) this.#go("consequences");
        else if (this.wallets.length > 1) this.#go("wallet");
        return;
      case "newPassphrase":
        // The phrase field starts empty again, and so does the model's copy.
        this.#dropSecrets();
        this.#go("phrase");
        return;
      case "failed":
        if (this.walletId) this.#go("phrase");
        else {
          this.#go("loading");
          void this.load();
        }
        return;
      default:
    }
  }

  /** Drops every secret (the flow closed). */
  dispose() {
    this.#dropSecrets();
  }

  #dropSecrets() {
    this.entry.clear();
    this.#bip39 = "";
  }

  #fail(message: string) {
    this.error = message;
    this.#go("failed");
  }

  #go(step: ForgotStep) {
    this.step = step;
    this.changed();
  }
}
