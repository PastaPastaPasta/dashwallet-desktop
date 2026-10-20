// History components (UX-SPEC C7, C8, C9, C25, C26): day-group cards of iOS transaction rows,
// and Activity's table mode (dash-qt's columns).

import { useId, type ReactNode } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend } from "../api/queries";
import type { HistorySortName } from "../api/generated/HistorySortName";
import { useDisplay, type Display } from "../format/display";
import { Avatar } from "../screens/contacts/Avatar";
import {
  contactAside,
  contactLetters,
  dayTitle,
  detailDate,
  groupByDay,
  isProblem,
  rowDate,
  rowParty,
  shownAmount,
  tableTypeTitle,
  timeOfDay,
  trailingStatus,
  txChip,
  txIcon,
  txTitle,
  weekday,
  type TxIcon,
} from "../format/tx";
import { AmountText, Chip, Icon, VisuallyHidden } from "./basics";
import type { UsedIcon } from "./icons";

const TX_ICON: Record<TxIcon, UsedIcon> = {
  received: "txReceived",
  sent: "txSent",
  internalTransfer: "txInternalTransfer",
  mixing: "txMixing",
  mining: "txMining",
  error: "txError",
};

export interface RowContext {
  display: Display;
  locale: string | undefined;
  now: Date;
}

/** Display unit, locale and clock the rows of a screen format with. */
export function useRowContext(network: NetworkInfo): RowContext {
  const backend = useBackend();
  return { display: useDisplay(network), locale: backend.locale, now: backend.now() };
}

/** Test id of a row: stable across engines and runs for the same transaction record. */
export const rowTestId = (row: TxRow) => `tx-row-${row.txid.slice(0, 10)}-${row.recordIndex}`;

/** What a row shows beyond its record (U-03 D14). Counterparty attribution (DP3-02) names the
 * contact through `txTitle`, and a named contact's avatar leads the row (DP3-07 D1). */
export interface RowDecor {
  title?: string;
  leading?: ReactNode;
}

function TxRowView({ row, ctx, onOpen, decorate }: { row: TxRow; ctx: RowContext; onOpen: (row: TxRow) => void; decorate?: (row: TxRow) => RowDecor }) {
  const decor = decorate?.(row);
  const title = decor?.title ?? txTitle(row);
  const chip = txChip(row.status);
  const trailing = trailingStatus(row.status);
  const date = rowDate(row);
  const shown = shownAmount(row.amount, row.txType);
  const amount = ctx.display.compact(shown.duffs, shown.signed);
  const letters = contactLetters(row.counterparty);
  const aside = contactAside(row, title);
  const leading =
    decor?.leading ??
    (letters !== null ? <Avatar letters={letters} identity={row.counterparty?.identity} /> : <Icon id={TX_ICON[txIcon(row.txType, row.status, row.amount)]} size={30} className="tx-row-icon" />);
  return (
    <li className="tx-row-item">
      <button type="button" className="tx-row" data-testid={rowTestId(row)} data-txid={row.txid} onClick={() => onOpen(row)}>
        {leading}
        <span className="tx-row-text">
          <span className={`tx-row-title ${isProblem(row.status) ? "is-problem" : ""}`}>{title}</span>
          {aside && <VisuallyHidden>{aside}</VisuallyHidden>}
          <span className="tx-row-subtitle">
            {date && <span className="tx-row-time">{timeOfDay(date, ctx.locale)}</span>}
            {chip && <Chip tone={isProblem(row.status) ? "danger" : "info"}>{chip}</Chip>}
          </span>
        </span>
        <span className="tx-row-trailing">
          {trailing && <span className="tx-row-status">{trailing}</span>}
          <AmountText className="tx-row-amount" text={amount} unitName={ctx.display.name} />
        </span>
      </button>
    </li>
  );
}

export function TxGroups({
  rows,
  ctx,
  onOpen,
  testIdPrefix,
  decorate,
}: {
  rows: TxRow[];
  ctx: RowContext;
  onOpen: (row: TxRow) => void;
  testIdPrefix: string;
  decorate?: (row: TxRow) => RowDecor;
}) {
  const groups = groupByDay(rows, rowDate);
  return (
    <div className="tx-groups">
      {groups.map((g, i) => (
        <TxGroup
          key={g.day?.getTime() ?? "undated"}
          day={g.day}
          rows={g.items}
          ctx={ctx}
          onOpen={onOpen}
          decorate={decorate}
          testId={`${testIdPrefix}-group-${i}`}
        />
      ))}
    </div>
  );
}

/** One day card (C8). A labelled group rather than a region: a long history must not fill the
 * landmark list with one entry per day. */
function TxGroup({
  day,
  rows,
  ctx,
  onOpen,
  decorate,
  testId,
}: {
  day: Date | null;
  rows: TxRow[];
  ctx: RowContext;
  onOpen: (row: TxRow) => void;
  decorate?: (row: TxRow) => RowDecor;
  testId: string;
}) {
  const titleId = useId();
  const title = day ? dayTitle(day, ctx.now, ctx.locale) : "Undated";
  return (
    <div className="tx-group card-l1" role="group" aria-labelledby={titleId} data-testid={testId}>
      <header className="tx-group-header">
        <h3 className="tx-group-day" id={titleId}>
          {title}
        </h3>
        {day && <span className="tx-group-weekday">{weekday(day, ctx.locale)}</span>}
      </header>
      <ul className="tx-group-rows" role="list">
        {rows.map((row) => (
          <TxRowView key={`${row.txid}:${row.recordIndex}`} row={row} ctx={ctx} onOpen={onOpen} decorate={decorate} />
        ))}
      </ul>
    </div>
  );
}

/** Table mode's status text (dash-qt's status column tooltip, shortened). */
export function tableStatus(status: TxRow["status"]): string {
  const chip = txChip(status);
  if (chip) return chip;
  if (status.kind === "immature") return `Immature, ${status.maturesIn ?? "?"} blocks left`;
  if (status.chainLocked) return "ChainLocked";
  return status.confirmations === 1 ? "1 confirmation" : `${status.confirmations} confirmations`;
}

const SORTS: Record<"date" | "amount", [HistorySortName, HistorySortName]> = {
  date: ["newestFirst", "oldestFirst"],
  amount: ["amountDescending", "amountAscending"],
};

function SortHeader({ column, title, sort, onSort, className }: { column: "date" | "amount"; title: string; sort: HistorySortName; onSort: (s: HistorySortName) => void; className?: string }) {
  const [first, second] = SORTS[column];
  const active = sort === first || sort === second;
  const ascending = sort === "oldestFirst" || sort === "amountAscending";
  return (
    <th scope="col" className={className} aria-sort={active ? (ascending ? "ascending" : "descending") : undefined}>
      <button type="button" className="sort-button" data-testid={`tx-sort-${column}`} onClick={() => onSort(sort === first ? second : first)}>
        {title}
        <span className="sort-mark" aria-hidden="true">
          {active ? (ascending ? "▲" : "▼") : ""}
        </span>
      </button>
    </th>
  );
}

/** Table mode (U-03 D13): dash-qt's columns. The Date cell's button opens the row's detail
 * (one control per row for the keyboard); a click anywhere on the row does too. */
export function TxTable({
  rows,
  ctx,
  onOpen,
  sort,
  onSort,
}: {
  rows: TxRow[];
  ctx: RowContext;
  onOpen: (row: TxRow) => void;
  sort: HistorySortName;
  onSort: (s: HistorySortName) => void;
}) {
  return (
    <div className="card-l1 table-card" data-testid="tx-table">
      <table className="data-table tx-table">
        <caption className="sr-only">Transactions</caption>
        <thead>
          <tr>
            <th scope="col">Status</th>
            <SortHeader column="date" title="Date" sort={sort} onSort={onSort} />
            <th scope="col">Type</th>
            <th scope="col">Address / Label</th>
            <SortHeader column="amount" title={`Amount (${ctx.display.name})`} sort={sort} onSort={onSort} className="num" />
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            const date = rowDate(row);
            const shown = shownAmount(row.amount, row.txType);
            return (
              <tr key={`${row.txid}:${row.recordIndex}`} className={isProblem(row.status) ? "is-problem" : undefined} onClick={() => onOpen(row)}>
                <td className="tx-table-status">{tableStatus(row.status)}</td>
                <td>
                  <button type="button" className="table-open" data-testid={`table-${rowTestId(row)}`} data-txid={row.txid} onClick={(e) => {
                    e.stopPropagation();
                    onOpen(row);
                  }}>
                    {date ? detailDate(date, ctx.locale) : "Undated"}
                  </button>
                </td>
                <td>{tableTypeTitle(row.txType)}</td>
                <td className="tx-table-party" title={row.address ?? undefined}>
                  {rowParty(row)}
                </td>
                <td className="num">{ctx.display.full(shown.duffs, shown.signed)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function EmptyState({ title, message, action, testId }: { title: string; message?: string; action?: ReactNode; testId: string }) {
  return (
    <div className="empty-state card-l1" data-testid={testId}>
      <Icon id="txAll" size={60} className="empty-state-icon" />
      <h3 className="empty-state-title">{title}</h3>
      {message && <p className="empty-state-message">{message}</p>}
      {action}
    </div>
  );
}

export function LoadingState({ text, testId }: { text: string; testId: string }) {
  return (
    <div className="loading-state" data-testid={testId} role="status">
      <span className="spinner" aria-hidden="true" />
      <span>{text}</span>
    </div>
  );
}
