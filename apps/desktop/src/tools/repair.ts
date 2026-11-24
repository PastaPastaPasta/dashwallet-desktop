// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): the Swift `RepairViewModel`,
// ported. Rescan from the wallet's birth, genesis or a height with progress and Cancel (U-05 D10);
// reset chain data, which asks first and stops and restarts sync around it (D11); remove
// unconfirmed transactions; the birth height; and "send all coins to yourself" for a registration
// or a credits top-up whose funding is unknown (D12, DP1-10 D16), and sending a move into Shielded's
// coins back to the wallet at a fee shown first; Finish transfers (DP1-09 D10).

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RescanStart } from "../api/generated/RescanStart";
import type { RescanStatus } from "../api/generated/RescanStatus";
import type { ShieldRepairBlocked } from "../api/generated/ShieldRepairBlocked";
import type { StuckFundingRefusalName } from "../api/generated/StuckFundingRefusalName";
import type { StuckShieldFunding } from "../api/generated/StuckShieldFunding";
import type { StuckTopUpFunding } from "../api/generated/StuckTopUpFunding";
import { messageTable, t } from "../i18n";
import { failureText, Model } from "../onboarding/model";
import { REG_COPY } from "../dashpay/registration";
import { forgetTopUp } from "../dashpay/topup";
import { recordUnreadableText, unreadableWallet } from "../history/recordUnreadable";
import { finishGrant } from "../dashpay/wizard";
import type { GrantModel } from "./grant";

export const RepairText = {
  get resetChainData() {
    return t("tools.repair.resetChainData");
  },
  get resetChainDataQuestion() {
    return t("tools.repair.resetChainDataQuestion");
  },
  get dropUnconfirmed() {
    return t("tools.repair.dropUnconfirmed");
  },
  get dropUnconfirmedQuestion() {
    return t("tools.repair.dropUnconfirmedQuestion");
  },
  dropped: (n: number) => t("tools.repair.dropped", { count: n }),
  get rescanUnavailable() {
    return t("tools.repair.rescanUnavailable");
  },
  progress: (current: number, target: number) => t("tools.repair.progress", { current: String(current), target: String(target) }),
  get birthHeightInvalid() {
    return t("tools.repair.birthHeightInvalid");
  },
  birthHeightSaved: (h: number) => t("tools.repair.birthHeightSaved", { height: String(h) }),
  get resetDone() {
    return t("tools.repair.resetDone");
  },
  get resolveStuck() {
    return t("tools.repair.resolveStuck");
  },
  /** An orphan funding step's line: its registration's row, and so its username, is gone. */
  get unnamedRegistration() {
    return t("tools.repair.unnamedRegistration");
  },
  get stuckUnreadable() {
    return t("tools.repair.stuckUnreadable");
  },
  get stuckTopUpsUnreadable() {
    return t("tools.repair.stuckTopUpsUnreadable");
  },
  get resolveStuckQuestion() {
    return t("tools.repair.resolveStuckQuestion");
  },
  resolved: (txid: string) => t("tools.repair.resolved", { txid }),
  /** A stuck top-up's line (DP1-10 D16): whose credits it tops up. */
  topUp: (name: string | null) => (name ? t("tools.repair.topUpNamed", { username: `@${name}` }) : t("tools.repair.topUpUnnamed")),
  get resolveStuckTopUpQuestion() {
    return t("tools.repair.resolveStuckTopUpQuestion");
  },
  resolvedTopUp: (txid: string) => t("tools.repair.resolvedTopUp", { txid }),
  /** A stuck move into Shielded's line: what happened, and what Repair does about it. */
  shield: (mixed: boolean) => (mixed ? t("tools.repair.shieldMixed") : t("tools.repair.shieldMain")),
  /** What Repair's payment for it spends: a move of mixed coins sends back only those coins, to the
   * main balance; a shield sends every spendable coin of the main balance. */
  shieldRepair: (mixed: boolean, coins: number, total: string, fee: string) =>
    t(mixed ? "tools.repair.shieldRepairMixed" : "tools.repair.shieldRepairMain", { count: coins, total, fee }),
  resolveStuckShieldQuestion: (mixed: boolean, coins: number, total: string, fee: string) =>
    t(mixed ? "tools.repair.resolveStuckShieldMixed" : "tools.repair.resolveStuckShieldMain", { count: coins, total, fee }),
  resolvedShield: (txid: string) => t("tools.repair.resolvedShield", { txid }),
  get stuckShieldsUnreadable() {
    return t("tools.repair.stuckShieldsUnreadable");
  },
};

const SHIELD_BLOCKED = messageTable("tools.repair.shieldBlockedReason", [
  "notReady",
  "spendUnsettled",
  "lockInputsSpent",
  "unattributedLock",
  "noKnownLock",
  "coinsHeld",
  "mixing",
  "noCoins",
  "otherReason",
]);

/** Why Repair cannot settle a move into Shielded now. A reason that can clear by itself says so; one
 * that cannot says that new moves into Shielded stay paused. */
export function shieldBlockedText(b: ShieldRepairBlocked): string {
  const why = b.reason === "other" ? SHIELD_BLOCKED.otherReason : SHIELD_BLOCKED[b.reason];
  return t(b.retryLater ? "tools.repair.shieldBlockedRetry" : "tools.repair.shieldBlockedPermanent", { why });
}

/** Copy for the engine's Repair refusals (the Swift `L10n.M2Errors`). */
export function repairErrorText(e: unknown): string {
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "spv_running":
      return t("tools.repair.errors.spvRunning");
    case "spv_not_running":
      return t("tools.repair.errors.spvNotRunning");
    case "rescan_in_progress":
      return RepairText.rescanUnavailable;
    case "height_out_of_range":
      return t("tools.repair.errors.heightOutOfRange");
    case "no_peers":
      return t("tools.repair.errors.noPeers");
    case "not_implemented":
      return t("tools.repair.errors.notImplemented");
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
        return t("tools.repair.stuckRefusal.rowsUnknown");
      case "notStuck":
        return t("tools.repair.stuckRefusal.notStuck");
      case "noMarker":
        return t("tools.repair.stuckRefusal.noMarker");
      case "callRunning":
        return t("tools.repair.stuckRefusal.callRunning");
      case "journalUnavailable":
        return t("tools.repair.stuckRefusal.journalUnavailable");
      case "spendUnsettled":
        return t("tools.repair.stuckRefusal.spendUnsettled", { txid: r.txid });
      case "lockInputsSpent":
        return t("tools.repair.stuckRefusal.lockInputsSpent", { txid: r.txid });
      case "missesSignedLock":
        return t("tools.repair.stuckRefusal.missesSignedLock", { txid: r.txid });
      case "unattributedLock":
        return t("tools.repair.stuckRefusal.unattributedLock", { txid: r.txid });
      case "noKnownLock":
        return t("tools.repair.stuckRefusal.noKnownLock");
    }
  })();
  return retryLater ? t("tools.repair.stuckRefusal.retryLater", { text }) : text;
}

/** A block height as typed: a whole number that fits a `u32`, else `null`. */
export function parseHeight(text: string): number | null {
  const t = text.trim();
  return /^\d+$/.test(t) && Number(t) <= 0xffff_ffff ? Number(t) : null;
}

/** A registration whose funding may or may not have gone out: Repair can settle it. */
export const isStuck = (r: RegistrationStatus) => r.waiting === "fundingUnknown";

const ORPHAN_STEP = /^registration\/[^/]+\/funding$/;

/** The funding step a registration's Repair names (`dispatchStatus`'s id). An orphan step (its
 * row gone) is reported with the step itself as its `draft` (m4 §2.2), so it is taken as it is
 * (f1, Sol r1 P1). */
export const fundingStep = (r: RegistrationStatus) => (ORPHAN_STEP.test(r.draft) ? r.draft : `registration/${r.draft}/funding`);

/** What "send all coins to yourself" settles: a registration's funding, a top-up's (D3), or a move
 * into Shielded's. */
export type StuckTarget = RegistrationStatus | StuckTopUpFunding | StuckShieldFunding;

export const isTopUp = (t: StuckTarget): t is StuckTopUpFunding => "identityIndex" in t;

export const isShield = (t: StuckTarget): t is StuckShieldFunding => "repair" in t;

/** A stuck top-up and the name of the identity it tops up (`null`: none known). */
export interface StuckTopUp {
  funding: StuckTopUpFunding;
  name: string | null;
}

export type Confirmation = "resetChainData" | "dropUnconfirmed" | "resolveStuck";

export type RepairState =
  | { kind: "idle" }
  | { kind: "confirming"; what: Confirmation; target?: StuckTarget }
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
  | "creditsStuckTopUpFundings"
  | "identities"
  | "stuckShieldFundings"
  | "listWallets"
>;

export class RepairModel extends Model {
  state: RepairState = { kind: "idle" };
  /** `null` while no rescan runs. */
  progress: RescanStatus | null = null;
  /** The wallet's registrations waiting on unknown funding. */
  stuck: RegistrationStatus[] = [];
  /** `registrations` failed (no Platform session, D12): the screen says so. */
  stuckUnreadable = false;
  /** The wallet's top-up fundings whose outcome is unknown (DP1-10 D16). */
  stuckTopUps: StuckTopUp[] = [];
  /** `stuck_top_up_fundings` failed while `registrations` read: the screen says so. */
  stuckTopUpsUnreadable = false;
  /** The wallet's moves into Shielded whose funding is unknown, each with Repair's price. */
  stuckShields: StuckShieldFunding[] = [];
  /** `stuck_shield_fundings` failed: the screen says so. */
  stuckShieldsUnreadable = false;

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
    const topUps = await this.#stuckTopUps();
    this.stuckTopUps = topUps ?? [];
    this.stuckTopUpsUnreadable = topUps === null && !this.stuckUnreadable;
    const shields = await this.api.stuckShieldFundings(this.walletId).catch(() => null);
    this.stuckShields = shields ?? [];
    this.stuckShieldsUnreadable = shields === null && !this.stuckUnreadable;
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

  /** Each of these asks first (D11, D12); `target` is the stuck funding to resolve. A move into
   * Shielded's is asked only with Repair's price, so the question can show its fee. */
  ask(what: Confirmation, target?: StuckTarget) {
    if (target && isShield(target) && !target.repair) return;
    if (!this.busy) this.#set({ kind: "confirming", what, target });
  }

  dismiss() {
    if (this.state.kind !== "working") this.#set({ kind: "idle" });
  }

  async confirm() {
    if (this.state.kind !== "confirming") return;
    const { what, target } = this.state;
    if (what === "resolveStuck") {
      this.#set({ kind: "idle" });
      return this.resolveStuck(target!);
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
   * covers the fee only, capped at the engine's maximum (U-05 D12). A move into Shielded's grant
   * is the fee its question showed: a dearer payment is refused, not paid. */
  async resolveStuck(t: StuckTarget) {
    if (this.busy) return;
    const since = this.grants.epoch;
    await this.#run(async () => {
      const maxDuffs = isShield(t) ? t.repair?.feeDuffs : (await this.api.feePolicy()).maxTxFee;
      if (maxDuffs === undefined) return { kind: "idle" };
      const sent = await this.grants.withGrant(
        { kind: "spend", maxDuffs },
        this.walletId,
        (grant) => this.api.resolveStuckFunding(this.walletId, "step" in t ? t.step : fundingStep(t), grant.id),
        since,
      );
      if (!sent) return { kind: "idle" };
      if (isShield(t)) return { kind: "done", text: RepairText.resolvedShield(sent.value.txid) };
      if (!isTopUp(t)) return { kind: "done", text: RepairText.resolved(sent.value.txid) };
      forgetTopUp(this.walletId);
      return { kind: "done", text: RepairText.resolvedTopUp(sent.value.txid) };
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

  /** The stuck top-ups with their identities' names; `null` when the read fails. */
  async #stuckTopUps(): Promise<StuckTopUp[] | null> {
    const fundings = await this.api.creditsStuckTopUpFundings(this.walletId).catch(() => null);
    if (fundings === null) return null;
    if (fundings.length === 0) return [];
    const identities = await this.api.identities(this.walletId).catch(() => []);
    return fundings.map((funding) => ({
      funding,
      name: identities.find((i) => i.identity === funding.identity || (funding.identity === null && i.index === funding.identityIndex))?.mainName ?? null,
    }));
  }

  async #run(f: () => Promise<RepairState>) {
    this.#set({ kind: "working" });
    try {
      this.#set(await f());
    } catch (e) {
      const unreadable = unreadableWallet(e);
      const text = unreadable === undefined ? repairErrorText(e) : recordUnreadableText(unreadable, await this.api.listWallets().catch(() => []));
      this.#set({ kind: "failed", text });
    }
    await this.refresh();
  }

  #set(state: RepairState) {
    this.state = state;
    this.changed();
  }
}
