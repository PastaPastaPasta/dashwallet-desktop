// The coin table and the selection's summary (U-03 D7, U-05 D19): Pay's coin control and
// Tools ▸ Coin Selection render these, over the same `txid:vout` selection. Coin Selection adds
// dash-qt's columns, sortable headers, the tree by address and a cell of row actions.

import { useId, type ReactNode } from "react";
import type { CoinSelectionSummary } from "../api/generated/CoinSelectionSummary";
import type { Utxo } from "../api/generated/Utxo";
import { pendingUnproven } from "../api/restoreHolds";
import type { Display } from "../format/display";
import { shortAddress } from "../format/tx";
import { t as tNow, useT } from "../i18n";
import { notSelectable, outpointKey } from "../screens/pay/model";
import { coinLabel, columnTitle, type CoinColumn, type CoinGroup, type CoinSort } from "../tools/coins";
import { AmountText } from "./basics";

/** Coin Selection's view of the table. */
export interface CoinTableDetail {
  sort: CoinSort;
  onSort: (column: CoinColumn) => void;
  /** Tree mode: the coins under one row per address. */
  groups: CoinGroup[] | null;
  onToggleGroup: (g: CoinGroup) => void;
  actions: (u: Utxo) => ReactNode;
}

interface Props {
  coins: Utxo[];
  /** The display settings (U-04 D4): the unit and its name. */
  shown: Display;
  selected: ReadonlySet<string>;
  onToggle: (key: string, on: boolean) => void;
  detail?: CoinTableDetail;
}

/** dash-qt's columns in Coin Selection, in its order. */
const DETAIL_COLUMNS: CoinColumn[] = ["amount", "label", "address", "rounds", "date", "confirmations"];

const NUMERIC = new Set<CoinColumn>(["amount", "rounds", "confirmations"]);

function status(u: Utxo): string {
  return notSelectable(u) ?? (u.coinjoinRounds !== null ? tNow("components.coins.mixed", { rounds: u.coinjoinRounds }) : u.isChange ? tNow("components.coins.change") : "");
}

const coinDate = (u: Utxo) => (u.timestamp === null ? "—" : new Date(u.timestamp * 1000).toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" }));

function SortHeader({ column, unit, detail }: { column: CoinColumn; unit: string; detail: CoinTableDetail }) {
  const current = detail.sort.column === column;
  const title = column === "amount" ? `${columnTitle("amount")} (${unit})` : columnTitle(column);
  return (
    <th scope="col" className={NUMERIC.has(column) ? "num" : undefined} aria-sort={current ? (detail.sort.ascending ? "ascending" : "descending") : undefined}>
      <button type="button" className="sort-header" data-testid={`coin-sort-${column}`} onClick={() => detail.onSort(column)}>
        {title}
        {current && <span aria-hidden="true">{detail.sort.ascending ? " ▲" : " ▼"}</span>}
      </button>
    </th>
  );
}

function CoinRow({ u, shown, selected, onToggle, detail }: Omit<Props, "coins"> & { u: Utxo }) {
  const t = useT();
  const key = outpointKey(u.outpoint);
  const why = notSelectable(u);
  const party = u.label || shortAddress(u.address);
  const on = selected.has(key);
  const heldId = useId();
  const held = pendingUnproven(u) ? t("components.coins.pendingUnproven.text") : null;
  return (
    <tr data-testid={`coin-${u.outpoint.txid.slice(0, 8)}-${u.outpoint.vout}`} className={detail?.groups ? "coin-child" : undefined}>
      <td>
        <input
          type="checkbox"
          aria-label={t("components.coins.use", { amount: shown.compact(u.amount), unit: shown.name, party })}
          checked={on}
          disabled={why !== null && !on}
          aria-describedby={held ? heldId : undefined}
          onChange={(e) => onToggle(key, e.target.checked)}
        />
        {held && (
          <span id={heldId} hidden>
            {held}
          </span>
        )}
      </td>
      <td className="num">
        <bdi dir="ltr">{shown.compact(u.amount)}</bdi>
      </td>
      {detail ? (
        <>
          <td>{coinLabel(u)}</td>
          <td className="mono" title={u.address}>
            {shortAddress(u.address)}
          </td>
          <td className="num">{u.coinjoinRounds ?? "—"}</td>
          <td>{coinDate(u)}</td>
        </>
      ) : (
        <td title={u.address}>{party}</td>
      )}
      <td className="num">{u.confirmations}</td>
      <td title={held ?? undefined}>{status(u)}</td>
      {detail && <td className="coin-actions">{detail.actions(u)}</td>}
    </tr>
  );
}

function GroupRow({ g, shown, selected, detail }: { g: CoinGroup; shown: Display; selected: ReadonlySet<string>; detail: CoinTableDetail }) {
  const t = useT();
  const selectable = g.coins.filter((u) => notSelectable(u) === null);
  const all = selectable.length > 0 && selectable.every((u) => selected.has(outpointKey(u.outpoint)));
  return (
    <tr className="coin-group" data-testid={`coin-group-${g.address}`}>
      <td>
        <input
          type="checkbox"
          aria-label={t("components.coins.useAll", { address: shortAddress(g.address) })}
          checked={all}
          disabled={selectable.length === 0}
          onChange={() => detail.onToggleGroup(g)}
        />
      </td>
      <td className="num">
        <bdi dir="ltr">{shown.compact(g.total)}</bdi>
      </td>
      <td>{g.label}</td>
      <td className="mono" title={g.address}>
        {shortAddress(g.address)}
      </td>
      <td colSpan={5}>{t("components.coins.count", { count: g.coins.length })}</td>
    </tr>
  );
}

export function CoinTable({ coins, shown, selected, onToggle, detail }: Props) {
  const t = useT();
  const unit = shown.name;
  const row = (u: Utxo) => <CoinRow key={outpointKey(u.outpoint)} u={u} shown={shown} selected={selected} onToggle={onToggle} detail={detail} />;
  return (
    <table className={detail ? "data-table coin-table coin-table-detail" : "data-table coin-table"} data-testid="coin-table">
      <caption className="sr-only">{t("components.coins.caption")}</caption>
      <thead>
        <tr>
          <th scope="col">
            <span className="sr-only">{t("components.coins.select")}</span>
          </th>
          {detail ? (
            <>
              {DETAIL_COLUMNS.map((c) => (
                <SortHeader key={c} column={c} unit={unit} detail={detail} />
              ))}
              <th scope="col">{t("components.coins.status")}</th>
              <th scope="col">
                <span className="sr-only">{t("tools.coins.columns.actions")}</span>
              </th>
            </>
          ) : (
            <>
              <th scope="col" className="num">
                {t("components.coins.amount", { unit })}
              </th>
              <th scope="col">{t("components.coins.party")}</th>
              <th scope="col" className="num">
                {t("tools.coins.columns.confirmations")}
              </th>
              <th scope="col">{t("components.coins.status")}</th>
            </>
          )}
        </tr>
      </thead>
      <tbody>
        {detail?.groups
          ? detail.groups.flatMap((g) => [<GroupRow key={g.address} g={g} shown={shown} selected={selected} detail={detail} />, ...g.coins.map(row)])
          : coins.map(row)}
      </tbody>
    </table>
  );
}

/** dash-qt's coin control summary; estimates carry "≈". */
export function CoinSummary({ summary: s, shown }: { summary: CoinSelectionSummary; shown: Display }) {
  const t = useT();
  const amount = (duffs: number) => <AmountText text={shown.full(duffs)} unitName={shown.name} glyph={false} />;
  return (
    <dl className="coin-summary" data-testid="coin-summary" aria-live="polite">
      <div>
        <dt>{t("components.coins.quantity")}</dt>
        <dd>{s.quantity}</dd>
      </div>
      <div>
        <dt>{t("tools.coins.columns.amount")}</dt>
        <dd>{amount(s.amount)}</dd>
      </div>
      <div>
        <dt>{t("components.coins.fee")}</dt>
        <dd>≈ {amount(s.fee)}</dd>
      </div>
      <div>
        <dt>{t("components.coins.afterFee")}</dt>
        <dd>≈ {amount(s.afterFee)}</dd>
      </div>
      <div>
        <dt>{t("components.coins.bytes")}</dt>
        <dd>≈ {s.bytes}</dd>
      </div>
      <div>
        <dt>{t("components.coins.summaryChange")}</dt>
        <dd>≈ {amount(s.change)}</dd>
      </div>
    </dl>
  );
}
