// The top-up flow (DASHPAY §4 `CreditsViewModel`'s top-up half, F8; DP1-10 T1–T8): the amount,
// the quote's confirm step, the call and its outcome, plus the pending top-ups' Finish and
// "Credits arrived" (D15). One per wallet, kept while the app runs, so the sheet can close while
// the call runs and reopen to its result (D14). Every grant goes through U-05's GrantModel.

import type { Backend } from "../api/backend";
import { hasCode, isAppError } from "../api/backend";
import { RESCAN_TEXT } from "../app/rescan";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CostTable } from "../api/generated/CostTable";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { TopUpFromPlatformOutcome } from "../api/generated/TopUpFromPlatformOutcome";
import type { TopUpFromPlatformQuote } from "../api/generated/TopUpFromPlatformQuote";
import type { TopUpOutcome } from "../api/generated/TopUpOutcome";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import { CREDITS_PER_DUFF } from "../api/pending";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { messageTable, t } from "../i18n";
import { creditCount } from "./profile";
import { COPY, withTable } from "./status";
import { platformHoldText } from "../api/platformHold";

export type TopUpApi = Pick<
  Backend,
  "creditsTopUp" | "creditsTopUpFromPlatform" | "creditsResumeTopUp" | "creditsResolveTopUp" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

/** D5: the presets (duffs) and the custom amount's floor. */
export const PRESETS = [5_000_000, 10_000_000] as const;
const MIN_TOP_UP = 1_000_000;

/** The custom amount's floor: 0.01 Dash, or the engine's if higher (D5). */
export const minTopUp = (costs: Pick<CostTable, "topUpMinDuffs"> | undefined) => Math.max(MIN_TOP_UP, costs?.topUpMinDuffs ?? 0);

/** What a top-up of `duffs` adds, as the quote estimates it: the lock in credits less the table's
 * fee, never below 0 (m4 §2.8). */
export const topUpCredits = (duffs: number, costs: Pick<CostTable, "creditsPerDuff" | "topUpFee">): bigint => {
  const credits = BigInt(duffs) * BigInt(costs.creditsPerDuff) - BigInt(costs.topUpFee);
  return credits > 0n ? credits : 0n;
};

/** D15: Finish's grant: the apply signs with the lock's funding key, which a funding budget
 * unlocks; nothing is spent (m4 §2.8). */
export const FINISH_GRANT: GrantRequest = { maxDuffs: 1, maxCredits: "0" };

const top = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(`dashpay.topUp.${prefix}`, keys);

export const TOPUP_COPY = withTable(messageTable("dashpay.topUp", ["title", "done", "close"]), {
  amount: withTable(top("amount", ["intro", "field", "invalid", "first", "next", "cancel"]), {
    get other() {
      return t("dashpay.topUp.amount.custom");
    },
    fieldUnit: (unit: string) => t("dashpay.topUp.amount.fieldUnit", { unit }),
    below: (min: string) => t("dashpay.topUp.amount.below", { min }),
  }),
  confirm: withTable(top("confirm", ["message", "amount", "fee", "total", "quoting", "back", "pay"]), {
    adds: (credits: string, estimate: string) => t("dashpay.topUp.confirm.adds", { credits, estimate }),
  }),
  running: top("running", ["title", "text"]),
  get repair() {
    return COPY.repair.action;
  },
});

export type TopUpStep = "amount" | "confirm" | "running" | "done" | "refused";

/** Which amount T1 has chosen; `max` only when paying from Platform. */
export type AmountChoice = { kind: "preset"; duffs: number } | { kind: "other"; text: string } | { kind: "max" };

/** Where the top-up is paid from (C-routes D-C3): the Dash wallet, or the Platform addresses. */
export type TopUpSource = "core" | "platform" | "shielded";

/** What a top-up from Platform asks for: `duffs` in credits, or everything (`null`). */
export const platformAmount = (duffs: number | null): WithdrawAmount => (duffs === null ? { kind: "all" } : { kind: "credits", credits: String(BigInt(duffs) * CREDITS_PER_DUFF) });

/** The outcome screens (T4–T7), and the result line of a pending row's Finish. */
export function outcomeText(o: TopUpOutcome, balance: (credits: string) => string): { title: string; text: string } {
  switch (o.state.kind) {
    case "applied":
    case "alreadyApplied": {
      const added = o.state.kind === "applied" && o.state.creditsAdded !== null ? creditCount(o.state.creditsAdded) : null;
      const now = o.state.balance === null ? null : balance(o.state.balance);
      const text =
        added === null
          ? now === null
            ? ""
            : t("dashpay.topUp.outcome.balance", { balance: now })
          : now === null
            ? t("dashpay.topUp.outcome.plus", { credits: added })
            : t("dashpay.topUp.outcome.plusBalance", { credits: added, balance: now });
      return { title: t("dashpay.topUp.outcome.added"), text };
    }
    case "proofWaiting":
      return { title: t("dashpay.topUp.outcome.sent.title"), text: t("dashpay.topUp.outcome.sent.text") };
    case "notApplied":
      return { title: t("dashpay.topUp.outcome.notApplied.title"), text: t("dashpay.topUp.outcome.notApplied.text") };
    case "consumptionUnknown":
      return { title: t("dashpay.topUp.outcome.unknown.title"), text: t("dashpay.topUp.outcome.unknown.text") };
  }
}

/** D17: a top-up's refusal, by code. */
export function topUpErrorText(e: unknown, amount: (duffs: number) => string, discreet = false): string {
  const held = platformHoldText(e);
  if (held) return held;
  if (!isAppError(e)) return failureText(e);
  switch (e.code) {
    case "credits.funding_insufficient": {
      if (e.data?.kind !== "duffs" || e.data.needed === null) return t("dashpay.topUp.error.short");
      const needed = amount(e.data.needed);
      return discreet ? t("dashpay.topUp.error.needed", { needed }) : t("dashpay.topUp.error.neededAvailable", { needed, available: amount(e.data.available) });
    }
    case "credits.below_minimum":
      return e.data?.kind === "limit" ? t("dashpay.topUp.error.belowMin", { min: amount(e.data.duffs) }) : t("dashpay.topUp.error.below");
    case "credits.top_up_pending":
      return t("dashpay.credits.pendingFirst");
    case "credits.coins_locked":
      return t("dashpay.topUp.error.coinsLocked");
    case "platform.broadcast_unknown":
      return t("dashpay.topUp.error.maybeSent");
    case "platform.will_be_sent":
      return t("dashpay.topUp.error.willBeSent");
    case "platform.context_unavailable":
      return t("dashpay.topUp.error.context");
    case "rescan_interrupted":
      return RESCAN_TEXT.interrupted;
    case "platform.trust_mismatch":
      return t("dashpay.topUp.error.trust");
    case "platform.unavailable":
    case "platform.timeout":
      return t("dashpay.topUp.error.unavailable");
    case "platform.signer_unavailable":
      return t("dashpay.topUp.error.signer");
    case "platform.grant_exceeded":
      return t("dashpay.topUp.error.grantExceeded");
    case "invalid_argument": {
      // The engine's detail, when there is one, is the advice (the 50-coin bound of a payment).
      const detail = e.message.replace(/^invalid argument:\s*/, "").trim();
      if (detail === "" || detail === e.code) return t("dashpay.topUp.error.stoppedWith", { code: e.code });
      const text = detail[0].toUpperCase() + detail.slice(1);
      return /[.!?]$/.test(text) ? text : `${text}.`;
    }
    default:
      return t("dashpay.topUp.error.stoppedWith", { code: e.code });
  }
}

/** T8: Back is offered when another amount may pass (D17). */
export const amountCanChange = (e: unknown) =>
  hasCode(e, "credits.funding_insufficient") || hasCode(e, "credits.below_minimum") || hasCode(e, "platform.grant_exceeded") || hasCode(e, "credits.quote_changed");

/** T8: the refusal routes to Tools ▸ Repair (D16). */
export const routesToRepair = (e: unknown) => hasCode(e, "platform.broadcast_unknown");

export const platformOp = (g: GrantRequest) => ({ kind: "platformOp" as const, maxDuffs: g.maxDuffs, maxCredits: g.maxCredits });

/** What a pending row's call came to: its result or refusal, shown under the row. */
export type RowResult = { txid: string; outcome: TopUpOutcome | null; error: unknown };

const settledListeners = new Set<() => void>();

/** D22: called whenever a top-up or a row's call ends, whatever screen is shown (events.ts
 * re-reads `["dashpay"]` from it). Returns the unsubscribe. */
export function onTopUpSettled(f: () => void): () => void {
  settledListeners.add(f);
  return () => settledListeners.delete(f);
}

export class TopUpFlow extends Model {
  /** The sheet is shown. */
  open = false;
  step: TopUpStep = "amount";
  /** The identity the flow tops up. */
  identity: string | null = null;
  choice: AmountChoice = { kind: "preset", duffs: PRESETS[0] };
  /** "Pay with": the Dash wallet every time the sheet opens, never kept (C3-8). */
  source: TopUpSource = "core";
  /** The amount the confirm step quotes; `null` with `source` Platform is Max. */
  duffs: number | null = null;
  outcome: TopUpOutcome | null = null;
  /** A top-up from Platform's result. */
  platformOutcome: TopUpFromPlatformOutcome | null = null;
  error: unknown = null;
  /** `top_up` runs (D14); the grant is asked for under it too. */
  running = false;
  /** A pending row's call runs: its txid. */
  rowRunning: string | null = null;
  rowResult: RowResult | null = null;
  /** Locks a call this session answered `consumptionUnknown`: "Credits arrived" is offered for
   * them before the library marks them final (D15, m4 §2.8 `resolve_top_up`). */
  readonly answeredUnknown = new Set<string>();
  readonly grants: GrantModel;

  constructor(
    private readonly api: TopUpApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** A result for `identity` that came while the sheet was hidden, not yet seen (D14): its page offers it.
   *  The flow is per wallet, so another main identity's page does not (D31): its Show would discard it. */
  unseenFor(identity: string): boolean {
    return !this.open && !this.running && this.identity === identity && (this.step === "done" || this.step === "refused");
  }

  /** Opens the sheet for `identity`: at T1, or where a running or unseen call is (D14). */
  begin(identity: string) {
    this.open = true;
    if (this.running || (this.identity === identity && (this.step === "done" || this.step === "refused"))) {
      this.changed();
      return;
    }
    this.identity = identity;
    this.step = "amount";
    this.choice = { kind: "preset", duffs: PRESETS[0] };
    this.source = "core";
    this.duffs = null;
    this.outcome = null;
    this.platformOutcome = null;
    this.error = null;
    this.changed();
  }

  choose(choice: AmountChoice) {
    this.choice = choice;
    this.changed();
  }

  /** "Pay with". Max is Platform's alone: leaving Platform goes back to the first preset. */
  chooseSource(source: TopUpSource) {
    if (this.running) return;
    this.source = source;
    if (source !== "platform" && this.choice.kind === "max") this.choice = { kind: "preset", duffs: PRESETS[0] };
    this.changed();
  }

  /** T1 → T2 for an amount the screen has checked against the floor; `null`: Max from Platform. */
  next(duffs: number | null) {
    this.duffs = duffs;
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    if (this.running) return;
    this.step = "amount";
    this.error = null;
    this.grants.cancel();
    this.changed();
  }

  /** Hides the sheet; a running call goes on and its result waits (D14). A result seen is
   * dropped. */
  close() {
    this.open = false;
    if (!this.running) {
      this.#dropResult();
      this.grants.cancel();
    }
    this.changed();
  }

  /** Drops an unseen result that a later call or Repair has overtaken (D14). */
  forget() {
    if (this.open || this.running) return;
    this.#dropResult();
    this.changed();
  }

  #dropResult() {
    if (this.step === "done" || this.step === "refused") this.step = "amount";
  }

  #settled(outcome: TopUpOutcome | null) {
    if (outcome?.state.kind === "consumptionUnknown") this.answeredUnknown.add(outcome.txid);
    for (const f of settledListeners) queueMicrotask(f);
  }

  /** Confirm & pay: the quote's grant, then `top_up` (one call at a time, D14). */
  async pay(grant: GrantRequest): Promise<void> {
    const { identity, duffs } = this;
    if (this.running || this.rowRunning !== null || identity === null || duffs === null || this.source !== "core") return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.platformOutcome = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.creditsTopUp(this.walletId, identity, duffs, g.id);
      });
      if (got) {
        this.outcome = got.value;
        this.step = "done";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
      // A cancelled passphrase ran nothing: nothing to read again.
      if (ran) this.#settled(this.step === "done" ? this.outcome : null);
      this.changed();
    }
  }

  /** Confirm & pay from Platform: the quote's grant, then `top_up_from_platform` of the quoted
   * credits, Max included, with the quote's input 0 (the fee source and its claim), so the engine
   * credits what Confirm showed and takes the fee as it showed, or refuses. */
  async payFromPlatform({ credits, grant, inputs }: Pick<TopUpFromPlatformQuote, "credits" | "grant" | "inputs">): Promise<void> {
    const { identity } = this;
    if (this.running || this.rowRunning !== null || identity === null || this.source !== "platform") return;
    let ran = false;
    this.running = true;
    this.error = null;
    this.platformOutcome = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(platformOp(grant), this.walletId, (g: AuthGrant) => {
        ran = true;
        this.step = "running";
        this.changed();
        return this.api.creditsTopUpFromPlatform(this.walletId, identity, { kind: "credits", credits }, inputs[0] ?? null, g.id);
      });
      if (got) {
        this.platformOutcome = got.value;
        this.step = "done";
      } else this.step = "confirm";
    } catch (e) {
      this.error = e;
      this.step = "refused";
    } finally {
      this.running = false;
      if (ran) this.#settled(null);
      this.changed();
    }
  }

  /** Finish (D15): `resume_top_up` under a one-duff funding budget. */
  async finish(row: PendingTopUp): Promise<void> {
    await this.#row(row.txid, async () => {
      const got = await this.grants.withGrant(platformOp(FINISH_GRANT), this.walletId, (g) => this.api.creditsResumeTopUp(this.walletId, row.txid, g.id));
      return got ? got.value : null;
    });
  }

  /** "Credits arrived" (D15): `resolve_top_up`, no grant; a proved row only. */
  async creditsArrived(row: PendingTopUp): Promise<void> {
    if (!row.proved) return;
    await this.#row(row.txid, async () => {
      await this.api.creditsResolveTopUp(this.walletId, row.txid);
      this.answeredUnknown.delete(row.txid);
      return null;
    });
  }

  dismissRow() {
    this.rowResult = null;
    this.changed();
  }

  async #row(txid: string, run: () => Promise<TopUpOutcome | null>) {
    if (this.rowRunning !== null || this.running) return;
    this.rowRunning = txid;
    this.rowResult = null;
    // A row's call overtakes the sheet's unseen result (D14).
    if (!this.open) this.#dropResult();
    this.changed();
    try {
      const outcome = await run();
      this.rowResult = outcome ? { txid, outcome, error: null } : null;
    } catch (e) {
      this.rowResult = { txid, outcome: null, error: e };
    } finally {
      this.rowRunning = null;
      this.#settled(this.rowResult?.outcome ?? null);
      this.changed();
    }
  }
}

const flows = new Map<string, TopUpFlow>();

/** The wallet's top-up flow, created on first use and kept while the app runs (D14). */
export function topUpFor(api: TopUpApi, walletId: string): TopUpFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new TopUpFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Repair resolved one of the wallet's top-up fundings: an unseen result is overtaken (D14). */
export function forgetTopUp(walletId: string) {
  flows.get(walletId)?.forget();
}

/** Tests: forget every flow (a fresh app). */
export function resetTopUps() {
  flows.clear();
}
