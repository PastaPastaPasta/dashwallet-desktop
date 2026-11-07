// "Move mixed coins" (IOS-057): the CoinJoin account's whole balance, fully mixed or
// not, to the wallet or into Shielded. To the wallet it is swept in chunks to a fresh receive
// address of the same wallet; the outcome shows what moved, what remains, and a chunk whose
// broadcast outcome is unknown, whose Try again sends the same signed transaction again. Into
// Shielded it is one asset lock of every coin, applied as any add to Shielded (a lock not applied
// yet is finished from the Shielded card); above 50 coins the coins are combined first inside the
// CoinJoin account, then moved once that round confirms. The plan step reads the plan,
// the confirm step asks for a spend grant of its total (U-05's GrantModel). Mixing has to stop
// first: the engine refuses while it runs and the sheet offers Stop Mixing. One flow per wallet,
// kept while the app runs, so the sheet can close while the call runs and reopen to its result.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
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
import { failureText, Model } from "../onboarding/model";
import { SHIELDED_COPY, shieldedErrorText, shieldedSettled } from "../shielded/flow";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { CoinJoinText } from "./model";

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

export const MOVE_COPY = {
  card: {
    title: "Move mixed coins",
    text: "Move the coins in your CoinJoin balance, mixed or not, back to your wallet's main balance.",
    /** Where the network has Shielded. */
    textShielded: "Move the coins in your CoinJoin balance, mixed or not, back to your wallet's main balance or into Shielded.",
    open: "Move to wallet",
    shield: "Move to Shielded",
  },
  title: "Move mixed coins",
  planning: "Reading your CoinJoin balance…",
  confirm: {
    message: "Your CoinJoin balance moves to a new address of this wallet. Coins that are not fully mixed yet lose the privacy mixing has given them so far.",
    fullyMixed: "Fully mixed",
    notFullyMixed: "Not fully mixed",
    combined: "These coins are combined on-chain, so the fully mixed amount loses some of its mixing privacy.",
    fee: "Network fee",
    receive: "You receive",
    transactions: (n: number) => (n === 1 ? "This takes one transaction." : `This takes ${n} transactions.`),
    waiting: (n: number) => (n === 1 ? "1 coin waits for a confirmation and stays in your CoinJoin balance." : `${n} coins wait for a confirmation and stay in your CoinJoin balance.`),
    cancel: "Cancel",
    move: "Move",
  },
  mixing: {
    text: "Stop mixing first. Moving coins while they mix would take coins a mixing session holds.",
    stop: "Stop Mixing",
  },
  running: {
    title: "Moving coins…",
    text: "You can close this: the result waits here.",
  },
  done: {
    title: "Coins moved",
    text: (received: string, fee: string) => `${received} is on its way to your wallet's main balance. Network fee: ${fee}.`,
  },
  partial: {
    title: "Not everything moved",
    text: (received: string, fee: string, remaining: string) =>
      `${received} is on its way to your wallet's main balance (network fee ${fee}); ${remaining} stayed in your CoinJoin balance. Try again to move the rest.`,
  },
  unknown: "One transaction may have reached the network. Its coins stay reserved until it confirms; the rest stayed in your CoinJoin balance. Try again sends the same transaction again.",
  resent: {
    sent: "Sent again. Its coins stay reserved until it confirms.",
    unknown: "The network has still not answered. Its coins stay reserved, and the wallet sends it again when it reconnects.",
    resolved: "It confirmed, or its coins were spent otherwise. Nothing is held for it any more.",
  } satisfies Record<MixedCoinsResendState, string>,
  shielded: {
    message: "Your whole CoinJoin balance goes into Shielded in one transaction. Dash Platform adds it to your private balance once the network has locked it.",
    fee: "Network fee, at most",
    poolFee: "Platform pool fee",
    gain: "Added to Shielded, at least",
  },
  combine: {
    text: (coins: number) =>
      `Your CoinJoin balance holds ${coins} coins, more than one move to Shielded takes (50). Combine them first: they stay in your CoinJoin balance, and once they confirm you move them to Shielded.`,
    after: "Coins after",
    go: "Combine",
    done: { title: "Coins combined", text: (fee: string) => `Your coins were combined inside your CoinJoin balance (network fee ${fee}). Move them to Shielded once they confirm.` },
    partial: "Not every coin was combined; the rest stayed as they were. Try again to combine them.",
  },
  unconfirmed: (n: number) =>
    n === 1 ? "1 coin of your CoinJoin balance is still confirming. A move to Shielded takes every coin: try again once it has." : `${n} coins of your CoinJoin balance are still confirming. A move to Shielded takes every coin: try again once they have.`,
  held: "Some of your mixed coins are held: locked in Coin Control, or by a move that may still reach the network. Unlock them or wait for it, then try again.",
  nothing: "There are no mixed coins to move.",
  otherMove: "Another move of your mixed coins is running. Try again when it ends.",
  changed: "The approval no longer covers this move. Review it again.",
  planChanged: "Your CoinJoin balance changed since you reviewed this move. Review it again.",
  close: "Close",
  again: "Try again",
} as const;

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

/** A plan or move refusal, by code; `amount` shows a duff figure a shielded refusal names. */
export function moveErrorText(e: unknown, amount: (duffs: number) => string = String): string {
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
    return shieldedErrorText(e, (c) => amount(creditsToDuffs(c)), amount, "add");
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
  /** The consolidation round a move to Shielded of more than 50 coins needs first. */
  consolidation: MixedCoinsConsolidationPlan | null = null;
  /** A move to the wallet's, or a consolidation round's (`combined`). */
  result: MixedCoinsSweepResult | null = null;
  /** `result` is a consolidation round's. */
  combined = false;
  /** A move to Shielded's. */
  shielded: ShieldOutcome | null = null;
  error: unknown = null;
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
  #fail(e: unknown) {
    this.error = e;
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
        this.#fail(e);
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
    await this.#run(
      { kind: "spend", maxDuffs: quote.totalDuffs },
      (g) => this.api.shieldMixedCoins(this.walletId, g.id),
      (outcome) => {
        this.shielded = outcome;
        shieldedSettled();
      },
      (e) => hasCode(e, "platform.grant_exceeded") || hasCode(e, SHIELDED_CODES.coinsChanged),
    );
  }

  /** The consolidation round a move to Shielded of more than 50 coins needs first. */
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
      this.changed();
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
