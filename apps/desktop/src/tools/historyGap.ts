// Home's notice for open wallets whose history starts before the block data this install keeps
// (engine `history_gap`): their earlier payments are missing until "Reset chain data and resync",
// the same reset Tools ▸ Repair offers. "Not now" hides it for those wallets until the app starts
// again.

import type { WalletHistoryGap } from "../api/generated/WalletHistoryGap";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { t } from "../i18n";
import { Model } from "../onboarding/model";
import { repairErrorText, RepairText, resetChainAndResync, type ResyncApi } from "./repair";

export const HistoryGapText = {
  get title() {
    return t("home.historyGap.title");
  },
  /** Names the wallet when there is one; a wallet the list does not have yet goes by its id. */
  body(gap: WalletHistoryGap, wallets: readonly WalletSummary[]): string {
    const height = String(gap.startHeight);
    if (gap.walletIds.length !== 1) return t("home.historyGap.wallets", { count: gap.walletIds.length, height });
    const id = gap.walletIds[0];
    return t("home.historyGap.oneWallet", { wallet: wallets.find((w) => w.walletId === id)?.name || id, height });
  },
  get action() {
    return t("home.historyGap.action");
  },
  get notNow() {
    return t("home.historyGap.notNow");
  },
  /** Repair's own label, so the two read as one action. */
  get reset() {
    return RepairText.resetChainDataButton;
  },
};

export type HistoryGapState = { kind: "shown" } | { kind: "confirming" } | { kind: "working" } | { kind: "failed"; text: string };

/** One notice per backend, so "Not now" and a reset under way outlive Home's mounts until the app
 * starts again. */
const models = new WeakMap<object, HistoryGapModel>();

export function historyGapModel(api: ResyncApi): HistoryGapModel {
  let model = models.get(api);
  if (!model) models.set(api, (model = new HistoryGapModel(api)));
  return model;
}

export class HistoryGapModel extends Model {
  state: HistoryGapState = { kind: "shown" };
  readonly #dismissed = new Set<string>();

  constructor(private readonly api: ResyncApi) {
    super();
  }

  /** Hides the notice for `walletIds`; a wallet not among them shows it again. */
  notNow(walletIds: readonly string[]) {
    if (this.state.kind === "working") return;
    walletIds.forEach((id) => this.#dismissed.add(id));
    this.#set({ kind: "shown" });
  }

  hides(walletIds: readonly string[]): boolean {
    return walletIds.every((id) => this.#dismissed.has(id));
  }

  ask() {
    if (this.state.kind === "shown" || this.state.kind === "failed") this.#set({ kind: "confirming" });
  }

  dismiss() {
    if (this.state.kind === "confirming") this.#set({ kind: "shown" });
  }

  /** Resets and resyncs; `true` once it has, so the caller reads `history_gap` again. */
  async confirm(): Promise<boolean> {
    if (this.state.kind !== "confirming") return false;
    this.#set({ kind: "working" });
    try {
      await resetChainAndResync(this.api);
    } catch (e) {
      this.#set({ kind: "failed", text: repairErrorText(e) });
      return false;
    }
    this.#set({ kind: "shown" });
    return true;
  }

  #set(state: HistoryGapState) {
    this.state = state;
    this.changed();
  }
}
