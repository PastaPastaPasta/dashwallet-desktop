// "Move mixed coins" (IOS-057): the CoinJoin account's whole balance, fully mixed or
// not, to the wallet or into Shielded. To the wallet it is swept in chunks to a fresh receive
// address of the same wallet; the outcome shows what moved, what remains, and a chunk whose
// broadcast outcome is unknown, whose Try again sends the same signed transaction again. Into
// Shielded it is one asset lock of every coin, applied as any add to Shielded (a lock not applied
// yet is finished from Unfinished transfers on Home); above `SHIELD_MOVE_MAX_COINS` (50) coins the coins are combined first inside the
// CoinJoin account, then moved once that round confirms. The plan step reads the plan,
// the confirm step asks for a spend grant of its total (U-05's GrantModel). Mixing has to stop
// first: the engine refuses while it runs and the sheet offers Stop Mixing. One flow per wallet,
// kept while the app runs, so the sheet can close while the call runs and reopen to its result.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import { toastPendingTransfer } from "../transfer/dispatch";
import type { ErrorCode } from "../api/generated/commands";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import type { MixedCoinsConsolidationPlan } from "../api/generated/MixedCoinsConsolidationPlan";
import type { MixedCoinsDestination } from "../api/generated/MixedCoinsDestination";
import type { MixedCoinsSweepPlan } from "../api/generated/MixedCoinsSweepPlan";
import type { MixedShieldQuote } from "../api/generated/MixedShieldQuote";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { creditsToDuffs, SHIELDED_CODES } from "../api/pending";
import type { MixedCoinsResendState } from "../api/generated/MixedCoinsResendState";
import type { MixedCoinsSweepResult } from "../api/generated/MixedCoinsSweepResult";
import { messageTable, t } from "../i18n";
import { en } from "../i18n/en";
import { failureText, Model } from "../onboarding/model";
import { SHIELDED_COPY, shieldedErrorText, shieldedSettled } from "../shielded/flow";
import { SHIELDED_SYNC_COPY, shieldedWait, type ShieldedWait } from "../shielded/syncing";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { CoinJoinText } from "./model";
import { platformHoldText } from "../api/platformHold";

export type MoveMixedApi = Pick<
  Backend,
  | "mixedCoinsSweepPlan"
  | "moveMixedCoins"
  | "resendMixedCoins"
  | "mixedCoinsConsolidationPlan"
  | "consolidateMixedCoins"
  | "shieldMixedQuote"
  | "shieldMixedCoins"
  | "stopMixing"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

/** The most coins one move to Shielded takes; more are combined first (the engine's limit). */
const SHIELD_MOVE_MAX_COINS = 50;

export const MOVE_COPY = {
  card: {
    get title() {
      return t("coinjoin.move.card.title");
    },
    get text() {
      return t("coinjoin.move.card.text");
    },
    /** Where the network has Shielded. */
    get textShielded() {
      return t("coinjoin.move.card.textShielded");
    },
    get open() {
      return t("coinjoin.move.card.open");
    },
    get shield() {
      return t("coinjoin.move.card.shield");
    },
  },
  get title() {
    return t("coinjoin.move.title");
  },
  get planning() {
    return t("coinjoin.move.planning");
  },
  confirm: {
    get message() {
      return t("coinjoin.move.confirm.message");
    },
    get fullyMixed() {
      return t("coinjoin.move.confirm.fullyMixed");
    },
    get notFullyMixed() {
      return t("coinjoin.move.confirm.notFullyMixed");
    },
    get combined() {
      return t("coinjoin.move.confirm.combined");
    },
    get fee() {
      return t("coinjoin.move.confirm.fee");
    },
    get receive() {
      return t("coinjoin.move.confirm.receive");
    },
    transactions: (n: number) => t("coinjoin.move.confirm.transactions", { count: n }),
    waiting: (n: number) => t("coinjoin.move.confirm.waiting", { count: n }),
    get cancel() {
      return t("common.cancel");
    },
    get move() {
      return t("coinjoin.move.confirm.move");
    },
  },
  mixing: {
    get text() {
      return t("coinjoin.move.mixing.text");
    },
    get stop() {
      return t("coinjoin.move.mixing.stop");
    },
  },
  running: {
    get title() {
      return t("coinjoin.move.running.title");
    },
    get text() {
      return t("coinjoin.move.running.text");
    },
  },
  done: {
    get title() {
      return t("coinjoin.move.done.title");
    },
    text: (received: string, fee: string) => t("coinjoin.move.done.text", { received, fee }),
  },
  partial: {
    get title() {
      return t("coinjoin.move.partial.title");
    },
    text: (received: string, fee: string, remaining: string) => t("coinjoin.move.partial.text", { received, fee, remaining }),
  },
  get unknown() {
    return t("coinjoin.move.unknown");
  },
  resent: messageTable(
    "coinjoin.move.resent",
    Object.keys(en.coinjoin.move.resent satisfies Record<MixedCoinsResendState, string>) as MixedCoinsResendState[],
  ) satisfies Readonly<Record<MixedCoinsResendState, string>>,
  shielded: {
    get message() {
      return t("coinjoin.move.shielded.message");
    },
    get fee() {
      return t("coinjoin.move.shielded.fee");
    },
    get poolFee() {
      return t("coinjoin.move.shielded.poolFee");
    },
    get gain() {
      return t("coinjoin.move.shielded.gain");
    },
  },
  combine: {
    text: (coins: number) => t("coinjoin.move.combine.text", { coins, limit: SHIELD_MOVE_MAX_COINS }),
    get after() {
      return t("coinjoin.move.combine.after");
    },
    get go() {
      return t("coinjoin.move.combine.go");
    },
    done: {
      get title() {
        return t("coinjoin.move.combine.done.title");
      },
      text: (fee: string) => t("coinjoin.move.combine.done.text", { fee }),
    },
    get partial() {
      return t("coinjoin.move.combine.partial");
    },
  },
  unconfirmed: (n: number) => t("coinjoin.move.unconfirmed", { count: n }),
  get held() {
    return t("coinjoin.move.held");
  },
  get nothing() {
    return t("coinjoin.move.nothing");
  },
  get otherMove() {
    return t("coinjoin.move.otherMove");
  },
  get changed() {
    return t("coinjoin.move.changed");
  },
  get planChanged() {
    return t("coinjoin.move.planChanged");
  },
  get close() {
    return t("coinjoin.move.close");
  },
  get again() {
    return t("coinjoin.move.again");
  },
};

export type MoveStep = "plan" | "confirm" | "combine" | "mixing" | "running" | "done" | "refused";

/** What a plan read found, for one destination. */
type MovePlan = { kind: "wallet"; plan: MixedCoinsSweepPlan } | { kind: "shielded"; quote: MixedShieldQuote } | { kind: "combine"; consolidation: MixedCoinsConsolidationPlan };

/** A sweep or consolidation plan's fee: what its chunks pay together. */
export const planFee = (p: { chunks: { fee: number }[] }) => p.chunks.reduce((sum, c) => sum + c.fee, 0);

/** What the confirm step shows the user receiving: the total less the fees. */
export const planReceives = (p: MixedCoinsSweepPlan) => p.total - planFee(p);

/** The coins a refusal names (`data.value`), or 0. */
const countOf = (e: unknown): number => {
  const d = (e as { data?: { kind?: string; value?: unknown } } | null)?.data;
  return d?.kind === "number" && typeof d.value === "number" ? d.value : 0;
};

/** Whether Move mixed to Shielded can start, as Add's: the reason it waits (`shieldedWaitText`) and
 * the unlock that would let it, or `null` when it can. */
export function moveShieldWait(status: ShieldedStatus | null | undefined): { why: string; unlock: "unlock" | "unlockFully" | null } | null {
  const wait = shieldedWait(status);
  if (wait === null) return null;
  return { why: SHIELDED_SYNC_COPY.wait(wait), unlock: wait === "locked" ? "unlock" : wait === "mixingOnly" ? "unlockFully" : null };
}

/** The sheet's refusal line: Dash Platform off said as at a send only where the move was sent. */
export const refusalText = (flow: Pick<MoveMixedFlow, "error" | "planFailed">, amount: (duffs: number) => string = String, wait: ShieldedWait | null = null): string =>
  platformHoldText(flow.error, !flow.planFailed) ?? moveErrorText(flow.error, amount, wait);

/** A plan or move refusal, by code; `amount` shows a duff figure a shielded refusal names, and `wait`
 * names why Shielded waits (`shieldedWait`). */
export function moveErrorText(e: unknown, amount: (duffs: number) => string = String, wait: ShieldedWait | null = null): string {
  if (hasCode(e, "coinjoin.nothing_to_move") || hasCode(e, SHIELDED_CODES.nothingToMove)) return MOVE_COPY.nothing;
  if (hasCode(e, "coinjoin.mixing_active") || hasCode(e, SHIELDED_CODES.mixingActive)) return MOVE_COPY.mixing.text;
  if (hasCode(e, "coinjoin.move_running") || hasCode(e, SHIELDED_CODES.moveRunning)) return MOVE_COPY.otherMove;
  if (hasCode(e, "coinjoin.vault_locked")) return CoinJoinText.vaultLocked;
  if (hasCode(e, "coinjoin.watch_only")) return CoinJoinText.watchOnly;
  if (hasCode(e, "coinjoin.grant_invalid")) return MOVE_COPY.changed;
  if (hasCode(e, SHIELDED_CODES.coinsUnconfirmed)) return MOVE_COPY.unconfirmed(countOf(e));
  if (hasCode(e, SHIELDED_CODES.coinsLocked)) return MOVE_COPY.held;
  if (hasCode(e, SHIELDED_CODES.coinsChanged)) return MOVE_COPY.planChanged;
  const code = (e as { code?: unknown } | null)?.code;
  if (typeof code === "string" && (code.startsWith("shielded.") || code.startsWith("platform."))) {
    return shieldedErrorText(e, (c) => amount(creditsToDuffs(c)), amount, "add", wait ?? false);
  }
  return failureText(e);
}

/** A thrown refusal the user answers with a fresh plan (a broadcast failure is the result's partial outcome). */
const RETRIED: ErrorCode[] = ["coinjoin.grant_invalid", "coinjoin.move_running", "coinjoin.vault_locked", "rescan_in_progress", SHIELDED_CODES.moveRunning, SHIELDED_CODES.coinsUnconfirmed, SHIELDED_CODES.coinsLocked, SHIELDED_CODES.coinsChanged];
export const canRetry = (e: unknown) => RETRIED.some((c) => hasCode(e, c));

/** The outcome of a move to Shielded, as an add to Shielded's (`credits` shows a credit figure). */
export function shieldResultText(o: ShieldOutcome, credits: (c: string) => string): { title: string; text: string } {
  const s = o.state;
  if (s.kind === "applied") return { title: SHIELDED_COPY.done.add.title, text: SHIELDED_COPY.done.add.text(credits(s.balance)) };
  if (s.kind === "proofWaiting") return SHIELDED_COPY.done.waiting;
  if (s.kind === "consumptionUnknown") return SHIELDED_COPY.done.checking;
  return { title: SHIELDED_COPY.done.notApplied.title, text: SHIELDED_COPY.done.notApplied.text(s.error) };
}

/** The outcome of a consolidation round. */
export function combineResultText(r: MixedCoinsSweepResult, amount: (duffs: number) => string): { title: string; text: string; kind: "done" | "partial" | "unknown" } {
  if (r.failureCode === null) return { kind: "done", title: MOVE_COPY.combine.done.title, text: MOVE_COPY.combine.done.text(amount(r.fee)) };
  if (r.unknownTxid !== null) return { kind: "unknown", title: MOVE_COPY.partial.title, text: MOVE_COPY.unknown };
  return { kind: "partial", title: MOVE_COPY.partial.title, text: MOVE_COPY.combine.partial };
}

/** How long Stop Mixing waits for the engine to stop: the last session winds down. */
const STOP_RETRIES = 20;
const STOP_RETRY_MS = 250;

/** The outcome of a move that ran: complete, partial, or with a chunk of unknown outcome. */
export function resultText(r: MixedCoinsSweepResult, amount: (duffs: number) => string): { title: string; text: string; kind: "done" | "partial" | "unknown" } {
  if (r.failureCode === null) return { kind: "done", title: MOVE_COPY.done.title, text: MOVE_COPY.done.text(amount(r.received), amount(r.fee)) };
  if (r.unknownTxid !== null) return { kind: "unknown", title: MOVE_COPY.partial.title, text: MOVE_COPY.unknown };
  return { kind: "partial", title: MOVE_COPY.partial.title, text: MOVE_COPY.partial.text(amount(r.received), amount(r.fee), amount(r.remaining)) };
}

/** The move card shows while the CoinJoin account holds a free coin, mixed or not (collaterals, change, a
 * consolidated output), and while a move runs or its result waits to be seen. */
export const showsMoveCard = (s: CoinJoinStatus, flow: Pick<MoveMixedFlow, "open" | "running" | "step">) =>
  s.unavailable?.kind !== "watchOnly" && (s.balances.coinjoinAccount > 0 || flow.open || flow.running || flow.step === "done");

export class MoveMixedFlow extends Model {
  open = false;
  step: MoveStep = "plan";
  /** Where the coins go. */
  destination: MixedCoinsDestination = "wallet";
  plan: MixedCoinsSweepPlan | null = null;
  /** A move to Shielded's quote. */
  shieldQuote: MixedShieldQuote | null = null;
  /** The consolidation round a move to Shielded of more than `SHIELD_MOVE_MAX_COINS` coins needs first. */
  consolidation: MixedCoinsConsolidationPlan | null = null;
  /** A move to the wallet's, or a consolidation round's (`combined`). */
  result: MixedCoinsSweepResult | null = null;
  /** `result` is a consolidation round's. */
  combined = false;
  /** A move to Shielded's. */
  shielded: ShieldOutcome | null = null;
  error: unknown = null;
  /** `error` refused the plan or quote, where nothing was being sent. */
  planFailed = false;
  /** A plan, stop, move or resend call runs. */
  running = false;
  /** What the last Try again on a chunk of unknown outcome came to. */
  resent: MixedCoinsResendState | null = null;
  /** The confirm step shows a fresh plan because the confirmed one's coins changed. */
  replanned = false;
  readonly grants: GrantModel;
  #epoch = 0;
  /** A move or Stop Mixing refusal that came while the sheet was closed. */
  #unseen = false;

  constructor(
    private readonly api: MoveMixedApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Opens the sheet: where a running move or its unseen outcome is, else at a fresh plan to `destination`. */
  begin(destination: MixedCoinsDestination = "wallet") {
    this.open = true;
    const unseen = this.#unseen;
    this.#unseen = false;
    if (this.running || this.step === "done" || (this.step === "refused" && unseen)) {
      this.changed();
      return;
    }
    this.destination = destination;
    void this.#plan();
  }

  /** A refusal: Stop Mixing while the wallet mixes, else its text. */
  #fail(e: unknown, planning = false) {
    this.error = e;
    this.planFailed = planning;
    this.step = mixingRefusal(e) ? "mixing" : "refused";
  }

  /** The plan to `destination`: a sweep plan, a shield quote, or the consolidation round a shield
   * of too many coins needs first. It only reads: the caller commits it if its epoch is current. */
  async #read(destination: MixedCoinsDestination): Promise<MovePlan> {
    if (destination === "wallet") return { kind: "wallet", plan: await this.api.mixedCoinsSweepPlan(this.walletId, "wallet") };
    try {
      return { kind: "shielded", quote: await this.api.shieldMixedQuote(this.walletId) };
    } catch (e) {
      if (!hasCode(e, SHIELDED_CODES.tooManyInputs)) throw e;
      return { kind: "combine", consolidation: await this.api.mixedCoinsConsolidationPlan(this.walletId) };
    }
  }

  /** Shows `read`, and only it: the other destinations' plans are cleared. */
  #commit(read: MovePlan) {
    this.plan = read.kind === "wallet" ? read.plan : null;
    this.shieldQuote = read.kind === "shielded" ? read.quote : null;
    this.consolidation = read.kind === "combine" ? read.consolidation : null;
    this.step = read.kind === "combine" ? "combine" : "confirm";
  }

  /** A fresh plan; `retries` more tries while mixing still winds down. */
  async #plan(retries = 0) {
    const epoch = ++this.#epoch;
    const { destination } = this;
    this.step = "plan";
    this.plan = null;
    this.shieldQuote = null;
    this.consolidation = null;
    this.result = null;
    this.combined = false;
    this.shielded = null;
    this.resent = null;
    this.replanned = false;
    this.error = null;
    this.changed();
    for (;;) {
      try {
        const read = await this.#read(destination);
        if (epoch !== this.#epoch) return;
        this.#commit(read);
      } catch (e) {
        if (epoch !== this.#epoch) return;
        if (retries-- > 0 && mixingRefusal(e)) {
          await new Promise((r) => setTimeout(r, STOP_RETRY_MS));
          if (epoch !== this.#epoch) return;
          continue;
        }
        this.#fail(e, true);
      }
      break;
    }
    this.changed();
  }

  /** Stop Mixing from the sheet, then a fresh plan once mixing has wound down. */
  async stopMixing() {
    if (this.running) return;
    this.running = true;
    this.changed();
    try {
      await this.api.stopMixing(this.walletId);
    } catch (e) {
      this.#fail(e);
      this.#unseen = !this.open;
      return;
    } finally {
      this.running = false;
      this.changed();
    }
    await this.#plan(STOP_RETRIES);
  }

  /** Move: a spend grant of the plan's total, then `move_mixed_coins`, or `shield_mixed_coins` to Shielded. */
  async move(): Promise<void> {
    if (this.destination === "shielded") return this.#shield();
    const { plan } = this;
    if (this.running || plan === null || this.step !== "confirm") return;
    await this.#run(
      { kind: "spend", maxDuffs: plan.total },
      (g) => this.api.moveMixedCoins(this.walletId, "wallet", plan.quoteId, g.id),
      (result) => (this.result = result),
      // The coins changed under the confirm step: show the new plan there.
      (e) => hasCode(e, "coinjoin.plan_changed"),
    );
  }

  /** Into Shielded: a spend grant of the quote's coins, then `shield_mixed_coins`. Coins that changed
   * since the quote (more than the grant, or one outside the plan) show a fresh quote. */
  async #shield(): Promise<void> {
    const quote = this.shieldQuote;
    if (this.running || quote === null || this.step !== "confirm") return;
    let ran = false;
    try {
      await this.#run(
        { kind: "spend", maxDuffs: quote.totalDuffs },
        (g) => {
          ran = true;
          return this.api.shieldMixedCoins(this.walletId, g.id);
        },
        (outcome) => (this.shielded = outcome),
        (e) => hasCode(e, "platform.grant_exceeded") || hasCode(e, SHIELDED_CODES.coinsChanged),
      );
    } finally {
      // A refused call can leave a step possibly sent too; a cancelled passphrase ran nothing.
      if (ran) shieldedSettled();
    }
  }

  /** The consolidation round a move to Shielded of more than `SHIELD_MOVE_MAX_COINS` coins needs first. */
  async combine(): Promise<void> {
    const plan = this.consolidation;
    if (this.running || plan === null || this.step !== "combine") return;
    await this.#run(
      { kind: "spend", maxDuffs: plan.total },
      (g) => this.api.consolidateMixedCoins(this.walletId, plan.quoteId, g.id),
      (result) => {
        this.result = result;
        this.combined = true;
      },
      (e) => hasCode(e, "coinjoin.plan_changed"),
    );
  }

  /** `call` under a spend grant of `budget`; `stale` refusals show a fresh plan where it was confirmed. */
  async #run<T>(budget: { kind: "spend"; maxDuffs: number }, call: (g: AuthGrant) => Promise<T>, done: (value: T) => void, stale: (e: unknown) => boolean): Promise<void> {
    const at = this.step;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant(budget, this.walletId, (g: AuthGrant) => {
        this.step = "running";
        this.changed();
        return call(g);
      });
      if (got) {
        done(got.value);
        this.step = "done";
      } else this.step = at;
    } catch (e) {
      if (stale(e)) {
        this.running = false;
        await this.#plan();
        this.replanned = this.step === at;
        this.changed();
        return;
      }
      this.#fail(e);
      this.#unseen = !this.open;
    } finally {
      this.running = false;
      if (this.step === "done" && this.shielded?.state.kind === "proofWaiting") {
        toastPendingTransfer(this.shielded.pendingDispatch, () => this.close());
      } else this.changed();
    }
  }

  /** Try again on a chunk of unknown outcome: its recorded bytes again, no grant, no new signature. */
  async resend(): Promise<void> {
    const txid = this.result?.unknownTxid ?? null;
    if (this.running || txid === null) return;
    this.running = true;
    this.changed();
    try {
      const sent = await this.api.resendMixedCoins(this.walletId, txid);
      this.resent = sent.find((s) => s.txid === txid)?.state ?? "resolved";
    } catch (e) {
      this.#fail(e);
      this.#unseen = !this.open;
    } finally {
      this.running = false;
      this.changed();
    }
  }

  /** Try again after a refusal or a partial move: a fresh plan. */
  again() {
    if (this.running) return;
    void this.#plan();
  }

  /** Hides the sheet; a running move goes on and its result waits. */
  close() {
    this.open = false;
    if (!this.running) {
      this.#epoch++;
      this.grants.cancel();
      if (this.step !== "running") this.step = "plan";
    }
    this.changed();
  }
}

/** The engine's refusal while the wallet mixes, from either move. */
const mixingRefusal = (e: unknown) => hasCode(e, "coinjoin.mixing_active") || hasCode(e, SHIELDED_CODES.mixingActive);

const flows = new Map<string, MoveMixedFlow>();

/** The wallet's flow, created on first use and kept while the app runs. */
export function moveMixedFor(api: MoveMixedApi, walletId: string): MoveMixedFlow {
  let f = flows.get(walletId);
  if (!f) {
    f = new MoveMixedFlow(api, walletId);
    flows.set(walletId, f);
  }
  return f;
}

/** Tests: forget every flow (a fresh app). */
export function resetMoveMixed() {
  flows.clear();
}
