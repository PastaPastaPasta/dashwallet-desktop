// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Transfer's states (POLISH 25) that need a world the reference wallet does not have: a main balance
// in more coins than one lock takes, no route at all, a lock blocking the next add, and the next call
// of a route ending refused or not applied. Every other state is the reference wallet's, driven from
// the sheet (e2e/transfer.spec.ts).

import type { ThemeName } from "../api/generated/ThemeName";
import type { Utxo } from "../api/generated/Utxo";
import type { World } from "./scenarios";

type Base = (theme: ThemeName | null) => World;

/** Coins of 0.1 DASH each for the main balance, `n` of them. */
function smallCoins(w: World, n: number): World {
  const main = w.wallets[0];
  const coins = Array.from({ length: n }, (_, i): Utxo => ({ ...main.coins[0], outpoint: { txid: (0xa0 + (i % 0x40)).toString(16).repeat(32), vout: i }, amount: 10_000_000, label: null, isChange: false }));
  main.coins = coins;
  const sum = coins.reduce((s, u) => s + u.amount, 0);
  const b = main.summary.balances!;
  main.summary = { ...main.summary, balances: { ...b, confirmed: sum, unconfirmed: 0, total: sum + b.immature + b.locked } };
  return w;
}

export function transferScenarios(base: Base) {
  return {
    /** A main balance of 60 coins of 0.1 DASH: Max needs more coins than one lock takes. */
    "transfer-many-coins": (t: ThemeName | null) => smallCoins(base(t), 60),
    /** Shielded not active and no Platform addresses: Transfer has nowhere to go and says why. */
    "transfer-none": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.shielded!.active = false;
      w.pending.platform = null;
      return w;
    },
    /** An add to Shielded whose lock waits for its proof: the next add (and its Max) is refused until it is finished. */
    "transfer-shield-pending": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.shielded!.pendingShields = [{ txid: "c0ffee02".repeat(8), duffs: 50_000_000, proved: false, consumptionUnknown: false }];
      return w;
    },
    /** The next add to Shielded locks, and its proof is still missing. */
    "transfer-shield-waiting": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.shielded!.shield = "proofWaiting";
      return w;
    },
    /** The next Shielded call (an add, a move to the Dash wallet) is refused by Platform. */
    "transfer-shielded-refused": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.shielded!.next = "refused";
      return w;
    },
    /** The next add to Platform locks, and Platform does not apply it. */
    "transfer-fund-not-applied": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.platform!.nextFund = "notApplied";
      return w;
    },
    /** The next add to Platform locks, and its proof is still missing. */
    "transfer-fund-waiting": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.platform!.nextFund = "proofWaiting";
      return w;
    },
    /** The next move from Platform is refused by Platform. */
    "transfer-move-refused": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.platform!.nextMove = "refused";
      return w;
    },
    /** The next move from Platform gets no answer: it stays pending. */
    "transfer-move-pending": (t: ThemeName | null) => {
      const w = base(t);
      w.pending.platform!.nextMove = "pending";
      return w;
    },
  };
}
