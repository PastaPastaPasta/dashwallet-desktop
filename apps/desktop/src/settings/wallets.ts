// Settings ▸ Wallets (UX-SPEC §4.17, QT-101, QT-114, IOS-110): the Swift
// `WalletManagementViewModel`, ported. The list is every registered wallet from
// `walletLoadStates` (D17); Open and Close also set the load-on-startup list, as dash-qt's File
// menu does, and the last open wallet cannot be closed.

import type { Backend } from "../api/backend";
import { LockModel } from "../onboarding/lock";
import { failureText, Model } from "../onboarding/model";
import { vaultCredential } from "./security";

export const WalletsText = {
  openAnotherFirst: "Open another wallet first.",
  openAnotherToClose: "Open another wallet first to close this one.",
  openAnotherToRemove: "Open another wallet first to remove this one: closed wallets cannot be reached with no wallet open.",
  openAnotherToCloseOrRemove: "Open another wallet first to close or remove this one.",
  noLoadStates: "Closed wallets are not shown: this engine does not list them yet.",
  nameRule: "A wallet name is 1 to 64 characters.",
  /** IOS-110. */
  removeQuestion: (name: string, watchOnly: boolean) =>
    watchOnly
      ? `Remove the watch-only wallet ${name} from this computer? Its history and settings are deleted. Its extended public key adds it again.`
      : `Remove the wallet ${name} from this computer? Its keys, history and settings are deleted. Its recovery phrase, or a backup or export you kept, brings it back.`,
  removed: (name: string) => `${name} was removed.`,
  watchOnlyHint: "Paste the account's extended public key (tpub or xpub). The wallet shows its balance and history; it cannot spend.",
  watchOnlyAdded: (name: string) => `${name} was added as a watch-only wallet.`,
  xpubTitle: "Extended public key",
  xpubHint: "Anyone with this key can see every address and payment of this account, but cannot spend.",
};

export interface WalletRow {
  walletId: string;
  name: string;
  loaded: boolean;
  loadOnStartup: boolean;
  watchOnly: boolean;
  /** Known for open wallets only (`listWallets`). */
  hasMnemonic: boolean | null;
}

const MAX_NAME = 64;

/** The trimmed name, or `null` when it breaks `rename_wallet`'s rule (1–64 characters). */
export function walletName(text: string): string | null {
  const t = text.trim();
  return t.length >= 1 && [...t].length <= MAX_NAME ? t : null;
}

/** An optional name field: `null` when blank, the trimmed name, `undefined` when it breaks the rule. */
export function optionalName(text: string): string | null | undefined {
  return text.trim() === "" ? null : (walletName(text) ?? undefined);
}

type ListApi = Pick<Backend, "walletLoadStates" | "listWallets" | "loadWallet" | "unloadWallet" | "setLoadOnStartup" | "renameWallet">;

export class WalletsModel extends Model {
  rows: WalletRow[] = [];
  loading = true;
  /** `walletLoadStates` did not answer: the open wallets only, without Open and Close. */
  fallback = false;
  error: string | null = null;
  /** The wallet whose call runs. */
  busy: string | null = null;
  #seq = 0;

  constructor(private readonly api: ListApi) {
    super();
  }

  async load() {
    const seq = ++this.#seq;
    try {
      const [states, summaries] = await Promise.all([this.api.walletLoadStates().catch(() => null), this.api.listWallets()]);
      if (seq !== this.#seq) return;
      const open = new Map(summaries.map((s) => [s.walletId, s]));
      this.fallback = states === null;
      this.rows = states
        ? states.map((s) => ({ ...s, hasMnemonic: open.get(s.walletId)?.hasMnemonic ?? null }))
        : summaries.map((s) => ({ walletId: s.walletId, name: s.name, loaded: true, loadOnStartup: true, watchOnly: s.watchOnly, hasMnemonic: s.hasMnemonic }));
      this.error = null;
    } catch (e) {
      if (seq !== this.#seq) return;
      this.error = failureText(e);
    }
    this.loading = false;
    this.changed();
  }

  get openCount(): number {
    return this.rows.filter((r) => r.loaded).length;
  }

  /** Close is refused for the last open wallet (D17). */
  canClose(row: WalletRow): boolean {
    return !this.fallback && row.loaded && this.openCount > 1;
  }

  /** Remove is refused for the last open wallet while closed ones exist: onboarding would open
   * with no way back to them (D26). */
  canRemove(row: WalletRow): boolean {
    return row.loaded && !(this.openCount === 1 && this.rows.some((r) => !r.loaded));
  }

  open(walletId: string) {
    return this.#run(walletId, async () => {
      await this.api.loadWallet(walletId);
      await this.api.setLoadOnStartup(walletId, true);
    });
  }

  async close(walletId: string): Promise<boolean> {
    const row = this.rows.find((r) => r.walletId === walletId);
    if (!row || !this.canClose(row)) {
      this.error = WalletsText.openAnotherFirst;
      this.changed();
      return false;
    }
    return this.#run(walletId, async () => {
      await this.api.unloadWallet(walletId);
      await this.api.setLoadOnStartup(walletId, false);
    });
  }

  setStartup(walletId: string, on: boolean) {
    return this.#run(walletId, () => this.api.setLoadOnStartup(walletId, on));
  }

  /** `false` when the name breaks the rule or the engine refused it (the error says why). */
  async rename(walletId: string, text: string): Promise<boolean> {
    const name = walletName(text);
    if (name === null) {
      this.error = WalletsText.nameRule;
      this.changed();
      return false;
    }
    return this.#run(walletId, () => this.api.renameWallet(walletId, name));
  }

  async #run(walletId: string, call: () => Promise<unknown>): Promise<boolean> {
    if (this.busy) return false;
    this.busy = walletId;
    this.error = null;
    this.changed();
    let ok = true;
    try {
      await call();
    } catch (e) {
      this.error = failureText(e);
      ok = false;
    }
    this.busy = null;
    await this.load();
    return ok;
  }
}

type RemoveApi = Pick<Backend, "lockStatus" | "unlock" | "unlockMixingOnly" | "authorize" | "removeWallet" | "revokeGrant">;

/** Remove wallet (IOS-110, D18): the confirmation, the passphrase for a `wipe` grant, then
 * `removeWallet`. */
export class RemoveWalletModel extends Model {
  step: "confirm" | "working" | "done" = "confirm";
  encrypted = true;
  error: string | null = null;
  readonly lock: LockModel;

  constructor(
    private readonly api: RemoveApi,
    readonly walletId: string,
  ) {
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

  get message(): string | null {
    return this.error ?? this.lock.message;
  }

  /** The passphrase is being checked or the wallet removed: the sheet stays open (D31). */
  get busy(): boolean {
    return this.#authorizing || this.lock.working || this.step === "working";
  }

  #authorizing = false;
  #cancelled = false;

  /** The sheet is gone: an authorization still running removes nothing. */
  cancel() {
    this.#cancelled = true;
  }

  async remove(passphrase: string) {
    if (this.step !== "confirm" || this.busy) return;
    let grant: string | null = null;
    const authorize = async (p: string) => {
      grant = (await this.api.authorize({ kind: "wipe" }, this.walletId, vaultCredential(this.encrypted, p))).id;
    };
    this.error = null;
    this.#authorizing = true;
    this.changed();
    const ok = this.encrypted
      ? await this.lock.attempt(passphrase, authorize)
      : await authorize("").then(
          () => true,
          (e) => {
            this.error = failureText(e);
            return false;
          },
        );
    this.#authorizing = false;
    if (!ok || !grant) return this.changed();
    // The sheet went away while the passphrase was checked: the wallet stays (D31).
    if (this.#cancelled) return void this.api.revokeGrant(grant).catch(() => {});
    this.step = "working";
    this.changed();
    try {
      await this.api.removeWallet(this.walletId, grant);
      this.step = "done";
    } catch (e) {
      this.api.revokeGrant(grant).catch(() => {});
      this.error = failureText(e);
      this.step = "confirm";
    }
    this.changed();
  }
}

/** Add watch-only wallet (QT-114): an account xpub and an optional name. */
export class WatchOnlyModel extends Model {
  error: string | null = null;
  working = false;
  added: { walletId: string; name: string } | null = null;

  constructor(private readonly api: Pick<Backend, "importWatchOnly">) {
    super();
  }

  async add(xpub: string, nameText: string) {
    if (this.working) return;
    const name = optionalName(nameText);
    if (name === undefined) return this.#fail(WalletsText.nameRule);
    if (xpub.trim() === "") return this.#fail("Paste an extended public key.");
    this.working = true;
    this.error = null;
    this.changed();
    try {
      const walletId = await this.api.importWatchOnly(xpub.trim(), { name, birthHeight: null, lookahead: null });
      this.added = { walletId, name: name ?? "The wallet" };
    } catch (e) {
      this.error = failureText(e);
    }
    this.working = false;
    this.changed();
  }

  #fail(message: string) {
    this.error = message;
    this.changed();
  }
}
