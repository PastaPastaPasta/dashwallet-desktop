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

/** The main balance as these coins only (`amount`, then anything else that differs from a confirmed
 * coin of account 0); `other` names the ones of another funding account. */
function maxCoins(w: World, coins: Array<Partial<Utxo> & { amount: number }>, other: number[] = []): World {
  const main = w.wallets[0];
  main.coins = coins.map((extra, vout): Utxo => ({ ...main.coins[0], outpoint: { txid: "d5".repeat(32), vout }, label: null, isChange: false, reserved: false, userLocked: false, ...extra }));
  main.otherAccounts = other.map((vout) => `${"d5".repeat(32)}:${vout}`);
  const sum = (f: (u: Utxo) => boolean) => main.coins.filter(f).reduce((s, u) => s + u.amount, 0);
  const confirmed = sum((u) => u.confirmations > 0);
  const unconfirmed = sum((u) => u.confirmations === 0);
  const b = main.summary.balances!;
  main.summary = { ...main.summary, balances: { ...b, confirmed, unconfirmed, total: confirmed + unconfirmed + b.immature + b.locked, coinjoin: sum((u) => u.coinjoinDenominated) } };
  return w;
}

/** A main balance of one coin of 0.00005 DASH, too little for either route's smallest amount once the
 * fee comes out; with `mixed`, a mixed coin of 0.1 DASH as well, which Max leaves in the wallet. */
const dustCoins = (w: World, mixed: boolean) =>
  maxCoins(w, [{ amount: 5_000 }, ...(mixed ? [{ amount: 10_000_100, coinjoinDenominated: true, coinjoinRounds: w.coinjoin.settings.rounds }] : [])]);

/** A coin of the wallet's own change, not confirmed or InstantSend-locked yet. */
const ARRIVING: Partial<Utxo> = { confirmations: 0, blockHeight: null, instantLocked: false, chainLocked: false, isChange: true };

export function transferScenarios(base: Base) {
  return {
    /** A main balance of 60 coins of 0.1 DASH: Max needs more coins than one lock takes. */
    "transfer-many-coins": (t: ThemeName | null) => smallCoins(base(t), 60),
    /** Max from the Dash wallet moves less than the smallest amount, and nothing is left out. */
    "transfer-max-dust": (t: ThemeName | null) => dustCoins(base(t), false),
    /** As "transfer-max-dust", with a mixed coin Max leaves in the Dash wallet. */
    "transfer-max-dust-left": (t: ThemeName | null) => dustCoins(base(t), true),
    /** Max from the Dash wallet has nothing final to move: its one coin, 0.002 DASH of its own change, is still arriving. */
    "transfer-max-arriving": (t: ThemeName | null) => maxCoins(base(t), [{ ...ARRIVING, amount: 200_000 }]),
    /** As "transfer-max-arriving", the coin being 0.00005 DASH: too little for either route's smallest amount even
     * once it has confirmed. */
    "transfer-max-arriving-small": (t: ThemeName | null) => maxCoins(base(t), [{ ...ARRIVING, amount: 5_000 }]),
    /** Max from the Dash wallet has nothing to move: its one coin of 0.002 DASH is another account's. */
    "transfer-max-nothing": (t: ThemeName | null) => maxCoins(base(t), [{ amount: 200_000 }], [0]),
    /** Max from the Dash wallet moves nothing, its one coin of 500 duffs being less than the fee, and
     * nothing is arriving or left out. */
    "transfer-max-tiny": (t: ThemeName | null) => maxCoins(base(t), [{ amount: 500 }]),
    /** No coin in the Dash wallet: Max is not offered. */
    "transfer-max-none": (t: ThemeName | null) => maxCoins(base(t), []),
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
    /** The reference wallet on mainnet: Shielded → Platform is off there (flag `unshield_to_platform`). */
    "transfer-mainnet": (t: ThemeName | null) => {
      const w = base(t);
      w.launch = { ...w.launch, network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" } };
      if (w.pending.networkChoice) w.pending.networkChoice = { ...w.pending.networkChoice, chosen: "mainnet", saved: true };
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
