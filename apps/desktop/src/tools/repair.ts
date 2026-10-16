// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): the Swift `RepairViewModel`,
// ported. Rescan from the wallet's birth, genesis or a height with progress and Cancel (U-05 D10);
// reset chain data, which asks first and stops and restarts sync around it (D11); remove
// unconfirmed transactions; the birth height; and "send all coins to yourself" for a registration
// whose funding is unknown (D12); Finish transfers (DP1-09 D10).

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RescanStart } from "../api/generated/RescanStart";
import type { RescanStatus } from "../api/generated/RescanStatus";
import type { StuckFundingRefusalName } from "../api/generated/StuckFundingRefusalName";
import { failureText, Model } from "../onboarding/model";
import { REG_COPY } from "../dashpay/registration";
import { finishGrant } from "../dashpay/wizard";
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
  resolveStuck: "Resolve stuck funding",
  stuckUnreadable: "This wallet's username registrations could not be read, so stuck funding cannot be checked here.",
  resolveStuckQuestion:
    "Send every coin of this wallet to yourself? Whether the registration's funding reached the network is settled either way; the payment costs only its fee.",
  resolved: (txid: string) => `All coins were sent to yourself (transaction ${txid}). The registration can go on once it is confirmed.`,
};

/** Copy for the engine's Repair refusals (the Swift `L10n.M2Errors`). */
export function repairErrorText(e: unknown): string {
  if (!isAppError(e)) return failureText(e);
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
      return failureText(e);
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

/** A block height as typed: a whole number that fits a `u32`, else `null`. */
export function parseHeight(text: string): number | null {
  const t = text.trim();
  return /^\d+$/.test(t) && Number(t) <= 0xffff_ffff ? Number(t) : null;
}

/** A registration whose funding may or may not have gone out: Repair can settle it. */
export const isStuck = (r: RegistrationStatus) => r.waiting === "fundingUnknown";

/** The funding step a registration's Repair names (`dispatchStatus`'s id). */
export const fundingStep = (r: RegistrationStatus) => `registration/${r.draft}/funding`;

export type Confirmation = "resetChainData" | "dropUnconfirmed" | "resolveStuck";

export type RepairState =
  | { kind: "idle" }
  | { kind: "confirming"; what: Confirmation; registration?: RegistrationStatus }
  | { kind: "working" }
  | { kind: "done"; text: string }
  | { kind: "failed"; text: string };

type Api = Pick<
  Backend,
  | "rescan"
  | "rescanProgress"
  | "cancelRescan"
  | "stopSpv"
  | "startSpv"
  | "resetChainData"
  | "setBirthHeight"
  | "dropUnconfirmed"
  | "registrations"
  | "resolveStuckFunding"
  | "feePolicy"
  | "registrationQuote"
  | "finishAssetLocks"
>;

export class RepairModel extends Model {
  state: RepairState = { kind: "idle" };
  /** `null` while no rescan runs. */
  progress: RescanStatus | null = null;
  /** The wallet's registrations waiting on unknown funding. */
  stuck: RegistrationStatus[] = [];
  /** `registrations` failed (no Platform session, D12): the screen says so. */
  stuckUnreadable = false;

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
      this.stuckUnreadable = false;
    } catch {
      // No Platform session (DashPay off, D12).
      this.stuck = [];
      this.stuckUnreadable = true;
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

  /** Each of these asks first (D11, D12); `registration` is the stuck one to resolve. */
  ask(what: Confirmation, registration?: RegistrationStatus) {
    if (!this.busy) this.#set({ kind: "confirming", what, registration });
  }

  dismiss() {
    if (this.state.kind !== "working") this.#set({ kind: "idle" });
  }

  async confirm() {
    if (this.state.kind !== "confirming") return;
    const { what, registration } = this.state;
    if (what === "resolveStuck") {
      this.#set({ kind: "idle" });
      return this.resolveStuck(registration!);
    }
    await this.#run(async () => {
      if (what === "dropUnconfirmed") return { kind: "done", text: RepairText.dropped(await this.api.dropUnconfirmed(this.walletId)) };
      // The engine resets only a stopped client; sync starts again whatever the stop or the reset
      // did (the stop can fail after SPV stopped, on a Platform drain, D11).
      // The first failure is the one shown; a restart that then fails too would only hide it.
      try {
        await this.api.stopSpv();
        await this.api.resetChainData();
      } catch (e) {
        await this.api.startSpv().catch(() => undefined);
        throw e;
      }
      await this.api.startSpv();
      return { kind: "done", text: RepairText.resetDone };
    });
  }

  /** A rescan from a typed height: a whole number, else refused before any call (D10). */
  async rescanFrom(text: string) {
    const height = parseHeight(text);
    if (height === null) return this.#set({ kind: "failed", text: RepairText.birthHeightInvalid });
    await this.rescan({ kind: "height", height });
  }

  /** IOS-113: a lower height schedules a rescan from it. */
  async setBirthHeight(text: string) {
    const height = parseHeight(text);
    if (height === null) return this.#set({ kind: "failed", text: RepairText.birthHeightInvalid });
    await this.#run(async () => {
      await this.api.setBirthHeight(this.walletId, height);
      return { kind: "done", text: RepairText.birthHeightSaved(height) };
    });
  }

  /** Sends every coin of the wallet to itself, so the funding is spent either way; the grant
   * covers the fee only, capped at the engine's maximum (U-05 D12). */
  async resolveStuck(r: RegistrationStatus) {
    if (this.busy) return;
    const since = this.grants.epoch;
    await this.#run(async () => {
      const { maxTxFee } = await this.api.feePolicy();
      const sent = await this.grants.withGrant(
        { kind: "spend", maxDuffs: maxTxFee },
        this.walletId,
        (grant) => this.api.resolveStuckFunding(this.walletId, fundingStep(r), grant.id),
        since,
      );
      return sent ? { kind: "done", text: RepairText.resolved(sent.value.txid) } : { kind: "idle" };
    });
  }

  /** "Finish transfers" (DP1-09 D10): resumes the asset locks no flow finished, registrations' and
   * top-ups', and says what came of them. */
  async finishTransfers() {
    if (this.busy) return;
    const since = this.grants.epoch;
    await this.#run(async () => {
      const caps = await finishGrant(this.api, this.walletId);
      const report = await this.grants.withGrant(
        { kind: "platformOp", maxDuffs: caps.maxDuffs, maxCredits: caps.maxCredits },
        this.walletId,
        (grant) => this.api.finishAssetLocks(this.walletId, grant.id),
        since,
      );
      return report ? { kind: "done", text: REG_COPY.finished(report.value) } : { kind: "idle" };
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
