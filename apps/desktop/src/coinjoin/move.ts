// "Move mixed coins" to the wallet (IOS-057, 0.2 MIX-SHIELD unit A): the CoinJoin account's whole
// balance, fully mixed or not, swept in chunks to a fresh receive address of the same wallet. The
// plan step reads the chunks and the mixed split, the confirm step asks for a spend grant of the
// plan's total (U-05's GrantModel), and the outcome shows what moved, what remains, and a chunk
// whose broadcast outcome is unknown. Mixing has to stop first: the engine refuses while it runs
// (`coinjoin.mixing_active`) and the sheet offers Stop Mixing. One flow per wallet, kept while the
// app runs, so the sheet can close while the call runs and reopen to its result.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { MixedCoinsSweepPlan } from "../api/generated/MixedCoinsSweepPlan";
import type { MixedCoinsSweepResult } from "../api/generated/MixedCoinsSweepResult";
import { failureText, Model } from "../onboarding/model";
import { GrantModel, PAYMENT_PROMPT } from "../tools/grant";
import { CoinJoinText } from "./model";

export type MoveMixedApi = Pick<
  Backend,
  "mixedCoinsSweepPlan" | "moveMixedCoins" | "stopMixing" | "authorize" | "revokeGrant" | "lockStatus" | "paymentAuthentication" | "unlock" | "unlockMixingOnly"
>;

export const MOVE_COPY = {
  card: {
    title: "Move mixed coins",
    text: "Move the coins in your CoinJoin balance, mixed or not, back to your wallet's main balance.",
    open: "Move to wallet",
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
    text: (moved: string) => `${moved} is on its way to your wallet's main balance.`,
  },
  partial: {
    title: "Not everything moved",
    text: (moved: string, remaining: string) => `${moved} moved; ${remaining} stayed in your CoinJoin balance. Try again to move the rest.`,
  },
  unknown: "One transaction may have reached the network. Its coins stay reserved until the wallet sees them spent; the rest stayed in your CoinJoin balance.",
  nothing: "There are no mixed coins to move.",
  changed: "Your CoinJoin balance changed. Review the move again.",
  close: "Close",
  again: "Try again",
} as const;

export type MoveStep = "plan" | "confirm" | "mixing" | "running" | "done" | "refused";

/** The plan's fee: what its chunks pay together. */
export const planFee = (p: MixedCoinsSweepPlan) => p.chunks.reduce((sum, c) => sum + c.fee, 0);

/** What the confirm step shows the user receiving: the total less the fees. */
export const planReceives = (p: MixedCoinsSweepPlan) => p.total - planFee(p);

/** A plan or move refusal, by code. */
export function moveErrorText(e: unknown): string {
  if (hasCode(e, "coinjoin.nothing_to_move")) return MOVE_COPY.nothing;
  if (hasCode(e, "coinjoin.mixing_active")) return MOVE_COPY.mixing.text;
  if (hasCode(e, "coinjoin.vault_locked")) return CoinJoinText.vaultLocked;
  if (hasCode(e, "coinjoin.watch_only")) return CoinJoinText.watchOnly;
  if (hasCode(e, "coinjoin.no_peers")) return "No peers to send to. Try again once the wallet is connected.";
  if (hasCode(e, "coinjoin.grant_invalid")) return MOVE_COPY.changed;
  if (hasCode(e, "coinjoin.broadcast_rejected")) return "The network refused the transaction. Your coins stayed in your CoinJoin balance.";
  return failureText(e);
}

/** A refusal the user answers with a fresh plan. */
export const canRetry = (e: unknown) => hasCode(e, "coinjoin.grant_invalid") || hasCode(e, "coinjoin.no_peers") || hasCode(e, "coinjoin.broadcast_rejected");

/** How long Stop Mixing waits for the engine to stop: the last session winds down. */
const STOP_RETRIES = 20;
const STOP_RETRY_MS = 250;

/** The outcome of a move that ran: complete, partial, or with a chunk of unknown outcome. */
export function resultText(r: MixedCoinsSweepResult, amount: (duffs: number) => string): { title: string; text: string; kind: "done" | "partial" | "unknown" } {
  if (r.failureCode === null) return { kind: "done", title: MOVE_COPY.done.title, text: MOVE_COPY.done.text(amount(r.moved)) };
  if (r.unknownTxid !== null) return { kind: "unknown", title: MOVE_COPY.partial.title, text: MOVE_COPY.unknown };
  return { kind: "partial", title: MOVE_COPY.partial.title, text: MOVE_COPY.partial.text(amount(r.moved), amount(r.remaining)) };
}

export class MoveMixedFlow extends Model {
  open = false;
  step: MoveStep = "plan";
  plan: MixedCoinsSweepPlan | null = null;
  result: MixedCoinsSweepResult | null = null;
  error: unknown = null;
  /** A plan, stop or move call runs. */
  running = false;
  readonly grants: GrantModel;
  #epoch = 0;

  constructor(
    private readonly api: MoveMixedApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.prompt = PAYMENT_PROMPT;
    this.grants.subscribe(() => this.changed());
  }

  /** Opens the sheet: where a running move or its unseen outcome is, else at a fresh plan. */
  begin() {
    this.open = true;
    if (this.running || this.step === "done" || this.step === "refused") {
      this.changed();
      return;
    }
    void this.#plan();
  }

  /** A refusal: Stop Mixing while the wallet mixes, else its text. */
  #fail(e: unknown) {
    this.error = e;
    this.step = hasCode(e, "coinjoin.mixing_active") ? "mixing" : "refused";
  }

  /** A fresh plan; `retries` more tries while mixing still winds down. */
  async #plan(retries = 0) {
    const epoch = ++this.#epoch;
    this.step = "plan";
    this.plan = null;
    this.result = null;
    this.error = null;
    this.changed();
    for (;;) {
      try {
        const plan = await this.api.mixedCoinsSweepPlan(this.walletId, "wallet");
        if (epoch !== this.#epoch) return;
        this.plan = plan;
        this.step = "confirm";
      } catch (e) {
        if (epoch !== this.#epoch) return;
        if (retries-- > 0 && hasCode(e, "coinjoin.mixing_active")) {
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
      this.error = e;
      this.step = "refused";
      return;
    } finally {
      this.running = false;
      this.changed();
    }
    await this.#plan(STOP_RETRIES);
  }

  /** Move: a spend grant of the plan's total, then `move_mixed_coins`. */
  async move(): Promise<void> {
    const { plan } = this;
    if (this.running || plan === null || this.step !== "confirm") return;
    this.running = true;
    this.error = null;
    this.changed();
    try {
      const got = await this.grants.withGrant({ kind: "spend", maxDuffs: plan.total }, this.walletId, (g: AuthGrant) => {
        this.step = "running";
        this.changed();
        return this.api.moveMixedCoins(this.walletId, "wallet", g.id);
      });
      if (got) {
        this.result = got.value;
        this.step = "done";
      } else this.step = "confirm";
    } catch (e) {
      this.#fail(e);
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
