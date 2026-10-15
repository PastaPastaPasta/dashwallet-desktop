// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): the Swift `RepairViewModel`,
// ported. Rescan from the wallet's birth, genesis or a height with progress and Cancel (U-05 D10);
// reset chain data, which asks first and stops and restarts sync around it (D11); remove
// unconfirmed transactions; the birth height; and "send all coins to yourself" for a registration
// whose funding is unknown (D12).

import { errorMessage, hasCode, isAppError, type Backend } from "../api/backend";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RescanStart } from "../api/generated/RescanStart";
import type { RescanStatus } from "../api/generated/RescanStatus";
import type { StuckFundingRefusalName } from "../api/generated/StuckFundingRefusalName";
import { Model } from "../onboarding/model";
import type { GrantModel } from "./grant";

export const RepairText = {
  resetChainData: "Reset chain data and resync",
  resetChainDataQuestion:
    "This deletes the downloaded block headers and filters and syncs them again. Your wallets, keys and settings are kept. Continue?",
  dropUnconfirmed: "Remove unconfirmed transactions",
  dropUnconfirmedQuestion:
    "Abandon every unconfirmed transaction that the network does not have and rescan? A transaction that reached the network may still confirm.",
  dropped: (n: number) => (n === 1 ? "1 transaction was removed; a rescan was scheduled." : `${n} transactions were removed; a rescan was scheduled.`),
  rescanUnavailable: "Rescan unavailable: Wallet is currently rescanning. Abort existing rescan or wait.",
  progress: (current: number, target: number) => `Rescanning… ${current} / ${target}`,
  birthHeightInvalid: "Enter a block height.",
  birthHeightSaved: (h: number) => `The wallet's birth height is now ${h}.`,
  resetDone: "The chain data was deleted; syncing again.",
  resolved: (txid: string) => `All coins were sent to yourself (transaction ${txid}). The registration can go on once it is confirmed.`,
};

/** Copy for the engine's Repair refusals (the Swift `L10n.M2Errors`). */
export function repairErrorText(e: unknown): string {
  if (!isAppError(e)) return errorMessage(e);
  switch (e.code) {
    case "spv_running":
      return "Stop syncing first.";
    case "spv_not_running":
      return "Sync is not running.";
    case "rescan_in_progress":
      return RepairText.rescanUnavailable;
    case "height_out_of_range":
      return "The height is above the current chain tip.";
    case "no_peers":
      return "No peers are connected. Try again once the wallet is online.";
    case "not_implemented":
      return "This feature is not available yet.";
    case "stuck_funding":
      return e.data?.kind === "stuckFunding" ? stuckFundingText(e.data.refusal, e.data.retryLater) : e.message;
    default:
      return errorMessage(e);
  }
}

/** Why "send all coins to yourself" was refused (E0-04 §6.5). */
export function stuckFundingText(r: StuckFundingRefusalName, retryLater: boolean): string {
  const text = ((): string => {
    switch (r.kind) {
      case "rowsUnknown":
        return "The wallet does not know this funding's coins yet.";
      case "notStuck":
        return "This funding is not stuck.";
      case "noMarker":
        return "There is no record of this funding.";
      case "callRunning":
        return "The funding is still being sent.";
      case "journalUnavailable":
        return "The payment record cannot be read right now.";
      case "spendUnsettled":
        return `An earlier repair (transaction ${r.txid}) is not confirmed yet.`;
      case "lockInputsSpent":
        return `The funding's coins were already spent by transaction ${r.txid}.`;
      case "missesSignedLock":
        return `The signed funding transaction ${r.txid} is missing.`;
      case "unattributedLock":
        return `Transaction ${r.txid} cannot be matched to this funding.`;
      case "noKnownLock":
        return "No funding transaction is known for this registration.";
    }
  })();
  return retryLater ? `${text} Try again later.` : text;
}

/** A registration whose funding may or may not have gone out: Repair can settle it. */
export const isStuck = (r: RegistrationStatus) => r.waiting === "fundingUnknown";

/** The funding step a registration's Repair names (`dispatchStatus`'s id). */
export const fundingStep = (r: RegistrationStatus) => `registration/${r.draft}/funding`;

export type Confirmation = "resetChainData" | "dropUnconfirmed";

export type RepairState =
  | { kind: "idle" }
  | { kind: "confirming"; what: Confirmation }
  | { kind: "working" }
  | { kind: "done"; text: string }
  | { kind: "failed"; text: string };

type Api = Pick<
  Backend,
  "rescan" | "rescanProgress" | "cancelRescan" | "stopSpv" | "startSpv" | "resetChainData" | "setBirthHeight" | "dropUnconfirmed" | "registrations" | "resolveStuckFunding" | "feePolicy"
>;

export class RepairModel extends Model {
  state: RepairState = { kind: "idle" };
  /** `null` while no rescan runs. */
  progress: RescanStatus | null = null;
  /** The wallet's registrations waiting on unknown funding. */
  stuck: RegistrationStatus[] = [];

  constructor(
    private readonly api: Api,
    readonly grants: GrantModel,
    readonly walletId: string,
  ) {
    super();
  }

  get rescanning(): boolean {
    return this.progress !== null;
  }

  get busy(): boolean {
    return this.state.kind === "working" || this.grants.pending !== null;
  }

  /** "Rescanning… current / target"; `null` while idle or unknown. */
  get progressText(): string | null {
    const p = this.progress;
    return p && p.currentHeight !== null && p.targetHeight !== null ? RepairText.progress(p.currentHeight, p.targetHeight) : null;
  }

  /** 0…1 within the rescan; `null` when unknown. */
  get progressFraction(): number | null {
    const p = this.progress;
    if (!p || p.currentHeight === null || p.targetHeight === null || p.targetHeight <= p.fromHeight) return null;
    return Math.min(1, Math.max(0, p.currentHeight - p.fromHeight) / (p.targetHeight - p.fromHeight));
  }

  /** The rescan's progress (the screen polls it each second while one runs) and the stuck
   * registrations. */
  async refresh() {
    try {
      this.progress = await this.api.rescanProgress();
    } catch (e) {
      this.progress = null;
      if (!hasCode(e, "not_implemented")) this.#set({ kind: "failed", text: repairErrorText(e) });
    }
    try {
      this.stuck = (await this.api.registrations(this.walletId)).filter(isStuck);
    } catch {
      // No Platform session (DashPay off): nothing to repair there.
      this.stuck = [];
    }
    this.changed();
  }

  async rescan(from: RescanStart) {
    if (this.busy) return;
    await this.#run(async () => {
      await this.api.rescan(from);
      return { kind: "idle" };
    });
  }

  /** `abortrescan`: stops and keeps what was found. */
  async cancelRescan() {
    try {
      await this.api.cancelRescan();
    } catch (e) {
      this.#set({ kind: "failed", text: repairErrorText(e) });
    }
    await this.refresh();
  }

  ask(what: Confirmation) {
    if (!this.busy) this.#set({ kind: "confirming", what });
  }

  dismiss() {
    if (this.state.kind !== "working") this.#set({ kind: "idle" });
  }

  async confirm() {
    if (this.state.kind !== "confirming") return;
    const what = this.state.what;
    await this.#run(async () => {
      if (what === "dropUnconfirmed") return { kind: "done", text: RepairText.dropped(await this.api.dropUnconfirmed(null)) };
      // The engine resets only a stopped client; sync starts again whatever the reset did.
      await this.api.stopSpv();
      try {
        await this.api.resetChainData();
      } finally {
        await this.api.startSpv();
      }
      return { kind: "done", text: RepairText.resetDone };
    });
  }

  /** IOS-113: a lower height schedules a rescan from it. */
  async setBirthHeight(text: string) {
    const t = text.trim();
    if (!/^\d+$/.test(t) || Number(t) > 0xffff_ffff) return this.#set({ kind: "failed", text: RepairText.birthHeightInvalid });
    const height = Number(t);
    await this.#run(async () => {
      await this.api.setBirthHeight(this.walletId, height);
      return { kind: "done", text: RepairText.birthHeightSaved(height) };
    });
  }

  /** Sends every coin of the wallet to itself, so the funding is spent either way; the grant
   * covers the fee only, capped at the engine's maximum (U-05 D12). */
  async resolveStuck(r: RegistrationStatus) {
    if (this.busy) return;
    await this.#run(async () => {
      const { maxTxFee } = await this.api.feePolicy();
      const grant = await this.grants.obtain({ kind: "spend", maxDuffs: maxTxFee }, this.walletId);
      if (grant === null) return { kind: "idle" };
      try {
        const { txid } = await this.api.resolveStuckFunding(this.walletId, fundingStep(r), grant.id);
        return { kind: "done", text: RepairText.resolved(txid) };
      } catch (e) {
        this.grants.revoke(grant);
        throw e;
      }
    });
  }

  async #run(f: () => Promise<RepairState>) {
    this.#set({ kind: "working" });
    try {
      this.#set(await f());
    } catch (e) {
      this.#set({ kind: "failed", text: repairErrorText(e) });
    }
    await this.refresh();
  }

  #set(state: RepairState) {
    this.state = state;
    this.changed();
  }
}
