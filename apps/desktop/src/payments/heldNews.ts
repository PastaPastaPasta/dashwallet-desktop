// Payments to look up after the unlock (a locked window reads nothing of the wallet): the ones
// that arrived while locked, and the bell's own, whose amounts and addresses the lock dropped.
// Bounded per wallet and in all: past a cap the newest are kept, and the take says so (the bell's
// own reads then answer for the rest; older payments are in Activity).

export const HELD_PER_WALLET = 50;
export const HELD_TOTAL = 100;
/** Most txids in one lookup. */
export const HELD_CHUNK = 25;

type Held = { walletId: string; txid: string };

/** One lookup: a wallet's txids, newest first. */
export type HeldRun = { walletId: string; txids: string[] };

export class HeldNews {
  /** In arrival order, oldest first. */
  #held: Held[] = [];
  #overflowed = false;

  constructor(
    private readonly perWallet = HELD_PER_WALLET,
    private readonly total = HELD_TOTAL,
    private readonly chunk = HELD_CHUNK,
  ) {}

  get size(): number {
    return this.#held.length;
  }

  /** Payments that just arrived: the newest. */
  hold(walletId: string, txids: readonly string[]) {
    const fresh = new Set(txids);
    this.#held = [...this.#held.filter((h) => h.walletId !== walletId || !fresh.has(h.txid)), ...[...fresh].map((txid) => ({ walletId, txid }))];
    this.#trim();
  }

  /** Payments from before the ones held, oldest first (the bell's at a lock, a drain cut short). */
  holdOlder(older: readonly Held[]) {
    const known = new Set(this.#held.map((h) => `${h.walletId}:${h.txid}`));
    this.#held = [...older.filter((h) => !known.has(`${h.walletId}:${h.txid}`)), ...this.#held];
    this.#trim();
  }

  /** Over a cap the oldest go. */
  #trim() {
    const counts = new Map<string, number>();
    const kept: Held[] = [];
    for (const h of [...this.#held].reverse()) {
      const n = counts.get(h.walletId) ?? 0;
      if (n >= this.perWallet || kept.length >= this.total) {
        this.#overflowed = true;
        continue;
      }
      counts.set(h.walletId, n + 1);
      kept.push(h);
    }
    this.#held = kept.reverse();
  }

  /** A wallet removed: its payments are not looked up. */
  drop(walletId: string) {
    this.#held = this.#held.filter((h) => h.walletId !== walletId);
  }

  /** A network switch or a wipe: none is. */
  clear() {
    this.#held = [];
    this.#overflowed = false;
  }

  /** Everything held, as lookups of one wallet's consecutive payments at most `chunk` long, the
   * oldest lookup first and each one newest first: the bell puts each lookup's payments above the
   * ones before, so it ends newest first and the newest are the ones it keeps. Empties it. */
  take(): { runs: HeldRun[]; overflowed: boolean } {
    const runs: HeldRun[] = [];
    let run: Held[] = [];
    const end = () => {
      if (run.length > 0) runs.push({ walletId: run[0].walletId, txids: run.map((h) => h.txid).reverse() });
      run = [];
    };
    for (const h of this.#held) {
      if (run.length === this.chunk || (run.length > 0 && run[0].walletId !== h.walletId)) end();
      run.push(h);
    }
    end();
    const overflowed = this.#overflowed;
    this.clear();
    return { runs, overflowed };
  }
}
