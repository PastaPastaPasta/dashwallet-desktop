// Tools ▸ Coin Selection (QT-069…072, QT-074): the Swift `CoinControlViewModel`'s list, ported.
// The coins are U-03's (`utxos`, the coin table Pay's coin control shows, U-05 D19); this adds
// dash-qt's list and tree views, sorting (this session, D8), CoinJoin coins hidden by default,
// locks, copy, and the selection's summary. "Pay with these coins" hands the selection to Pay.

import { hasCode, type Backend } from "../api/backend";
import type { CoinSelectionSummary } from "../api/generated/CoinSelectionSummary";
import type { Utxo } from "../api/generated/Utxo";
import { format, type Unit } from "../format/amount";
import { failureText, Model } from "../onboarding/model";
import { DEFAULT_TARGET_BLOCKS, notSelectable, outpointKey, outpointOf } from "../screens/pay/model";

export type CoinColumn = "amount" | "label" | "address" | "rounds" | "date" | "confirmations";

/** dash-qt's column titles; "Received with label" and "Received with address" are shortened to
 * fit the window (D22). */
export const COLUMN_TITLE: Record<CoinColumn, string> = {
  amount: "Amount",
  label: "Label",
  address: "Address",
  rounds: "Mixing Rounds",
  date: "Date",
  confirmations: "Confirmations",
};

export const CoinText = {
  change: "(change)",
  noLabel: "(no label)",
  locked: (n: number) => `(${n} locked)`,
  showAll: "Show all coins",
  hideCoinJoin: "Hide CoinJoin coins",
  unselected: "Some coins were unselected because they were spent.",
  tolerance: (duffs: number) => `Can vary +/- ${duffs} duffs per input.`,
  unavailable: "The summary is not available yet.",
};

export const coinLabel = (u: Utxo) => (u.isChange ? CoinText.change : u.label || CoinText.noLabel);

/** A coin's amount as dash-qt copies it: in the display unit (U-04 D4), no separators. */
export const plainAmount = (duffs: number, unit: Unit = "DASH") => format(unit, duffs, false, "never", false);

const VALUE: Record<CoinColumn, (u: Utxo) => number | string> = {
  amount: (u) => u.amount,
  label: coinLabel,
  address: (u) => u.address,
  rounds: (u) => u.coinjoinRounds ?? -1,
  date: (u) => u.timestamp ?? Number.MAX_SAFE_INTEGER,
  confirmations: (u) => u.confirmations,
};

export interface CoinSort {
  column: CoinColumn;
  ascending: boolean;
}

/** dash-qt's default: largest first. */
export const DEFAULT_SORT: CoinSort = { column: "amount", ascending: false };

export function sortCoins(coins: Utxo[], { column, ascending }: CoinSort): Utxo[] {
  const v = VALUE[column];
  return coins.toSorted((a, b) => {
    const x = v(a);
    const y = v(b);
    const c = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y), "en", { sensitivity: "base" });
    return (ascending ? c : -c) || outpointKey(a.outpoint).localeCompare(outpointKey(b.outpoint));
  });
}

export interface CoinGroup {
  address: string;
  label: string;
  total: number;
  coins: Utxo[];
}

/** Tree mode: one group per address, with its total, in the list's order. */
export function groupCoins(coins: Utxo[]): CoinGroup[] {
  const groups = new Map<string, CoinGroup>();
  for (const u of coins) {
    const g = groups.get(u.address) ?? { address: u.address, label: coinLabel(u), total: 0, coins: [] };
    g.total += u.amount;
    g.coins.push(u);
    groups.set(u.address, g);
  }
  return [...groups.values()];
}

export type CopyField = "address" | "label" | "amount" | "outpoint";

export function copyText(u: Utxo, field: CopyField, unit: Unit = "DASH"): string {
  switch (field) {
    case "address":
      return u.address;
    case "label":
      return u.label ?? "";
    case "amount":
      return plainAmount(u.amount, unit);
    case "outpoint":
      return outpointKey(u.outpoint);
  }
}

type Api = Pick<Backend, "lockOutpoints" | "unlockOutpoints" | "coinSelectionSummary">;

export class CoinSelectionModel extends Model {
  coins: Utxo[] | null = null;
  mode: "list" | "tree" = "list";
  sort: CoinSort = DEFAULT_SORT;
  showCoinJoin = false;
  /** Selected coins, `txid:vout`, in selection order (Pay's `Advanced.selected`). */
  selected: string[] = [];
  /** QT-074: selected coins were spent elsewhere and unselected. */
  unselectedNotice = false;
  summary: CoinSelectionSummary | null = null;
  /** The engine cannot compute the summary yet: shown as unavailable, never as zeros. */
  summaryUnavailable = false;
  error: string | null = null;
  #summarySeq = 0;

  constructor(
    private readonly api: Api,
    readonly walletId: string,
    /** Reads the coins again (the screen's query) after a lock changes them. */
    private readonly reload: () => Promise<Utxo[]>,
  ) {
    super();
  }

  /** The coins shown: CoinJoin coins only when asked for, in the chosen order. */
  get visible(): Utxo[] {
    const coins = (this.coins ?? []).filter((u) => this.showCoinJoin || !u.coinjoinDenominated);
    return sortCoins(coins, this.sort);
  }

  get groups(): CoinGroup[] {
    return groupCoins(this.visible);
  }

  get lockedCount(): number {
    return (this.coins ?? []).filter((u) => u.userLocked).length;
  }

  get lockedText(): string | null {
    return this.lockedCount ? CoinText.locked(this.lockedCount) : null;
  }

  get coinJoinToggleTitle(): string {
    return this.showCoinJoin ? CoinText.hideCoinJoin : CoinText.showAll;
  }

  /** The coins from `utxos`; a selected coin that is gone or no longer selectable is unselected,
   * with dash-qt's notice. */
  setCoins(coins: Utxo[]) {
    this.coins = coins;
    const usable = new Set(coins.filter((u) => notSelectable(u) === null).map((u) => outpointKey(u.outpoint)));
    const kept = this.selected.filter((k) => usable.has(k));
    if (kept.length !== this.selected.length) {
      this.selected = kept;
      this.unselectedNotice = true;
      void this.#summarize();
    }
    this.changed();
  }

  setMode(mode: "list" | "tree") {
    this.mode = mode;
    this.changed();
  }

  /** A column header: that column, its direction flipped when it is the current one. */
  sortBy(column: CoinColumn) {
    this.sort = this.sort.column === column ? { column, ascending: !this.sort.ascending } : { column, ascending: column !== "amount" };
    this.changed();
  }

  setShowCoinJoin(show: boolean) {
    this.showCoinJoin = show;
    this.changed();
  }

  toggle(key: string) {
    const u = this.coins?.find((c) => outpointKey(c.outpoint) === key);
    if (!u) return;
    if (this.selected.includes(key)) this.selected = this.selected.filter((k) => k !== key);
    else if (notSelectable(u) === null) this.selected = [...this.selected, key];
    else return;
    this.unselectedNotice = false;
    void this.#summarize();
    this.changed();
  }

  /** dash-qt "(un)select all": every selectable shown coin, or none when all are selected. */
  toggleAll() {
    this.toggleCoins(this.visible);
  }

  /** A tree's address row: its selectable coins, or none of them when all are selected. */
  toggleCoins(coins: Utxo[]) {
    const selectable = coins.filter((u) => notSelectable(u) === null).map((u) => outpointKey(u.outpoint));
    const all = selectable.length > 0 && selectable.every((k) => this.selected.includes(k));
    this.selected = all ? this.selected.filter((k) => !selectable.includes(k)) : [...new Set([...this.selected, ...selectable])];
    this.unselectedNotice = false;
    void this.#summarize();
    this.changed();
  }

  async lock(keys: string[]) {
    await this.#locks(keys, []);
  }

  async unlock(keys: string[]) {
    await this.#locks([], keys);
  }

  /** dash-qt "Toggle lock state": every shown coin's lock flips. */
  async toggleLocks() {
    const shown = this.visible;
    await this.#locks(
      shown.filter((u) => !u.userLocked && !u.reserved).map((u) => outpointKey(u.outpoint)),
      shown.filter((u) => u.userLocked).map((u) => outpointKey(u.outpoint)),
    );
  }

  async #locks(lock: string[], unlock: string[]) {
    try {
      if (lock.length) {
        await this.api.lockOutpoints(this.walletId, lock.map(outpointOf));
        // A locked coin cannot stay selected; that is the user's doing, not a spend: no notice.
        // Dropped at once, so a failed unlock after it leaves no locked coin selected.
        this.selected = this.selected.filter((k) => !lock.includes(k));
      }
      if (unlock.length) await this.api.unlockOutpoints(this.walletId, unlock.map(outpointOf));
      this.error = null;
    } catch (e) {
      this.error = failureText(e);
    }
    const coins = await this.reload().catch(() => this.coins ?? []);
    this.coins = coins;
    void this.#summarize();
    this.changed();
  }

  /** The summary of the selection at the recommended fee, with nothing paid yet. */
  async #summarize() {
    const seq = ++this.#summarySeq;
    if (this.selected.length === 0) {
      this.summary = null;
      this.summaryUnavailable = false;
      return this.changed();
    }
    try {
      const s = await this.api.coinSelectionSummary(this.walletId, this.selected.map(outpointOf), [], { kind: "recommended", targetBlocks: DEFAULT_TARGET_BLOCKS });
      if (seq !== this.#summarySeq) return;
      this.summary = s;
      this.summaryUnavailable = false;
      if (s.unavailable.length) {
        const gone = new Set(s.unavailable.map(outpointKey));
        this.selected = this.selected.filter((k) => !gone.has(k));
        this.unselectedNotice = true;
      }
    } catch (e) {
      if (seq !== this.#summarySeq) return;
      this.summary = null;
      this.summaryUnavailable = hasCode(e, "not_implemented");
      if (!this.summaryUnavailable) this.error = failureText(e);
    }
    this.changed();
  }
}
