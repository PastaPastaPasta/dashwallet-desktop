// History components (UX-SPEC C7, C8, C9, C25, C26): day-group cards of iOS transaction rows,
// and Activity's table mode (dash-qt's columns). With a price shown (MP-01), each amount has its value now
// under it, and the table a value column; discreet mode leaves them out.

import { useId, useState, type ReactNode } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxRow } from "../api/generated/TxRow";
import { useBackend } from "../api/queries";
import type { HistorySortName } from "../api/generated/HistorySortName";
import { FiatLine, useFiat, type Fiat } from "../fiat/rates";
import { useDisplay, type Display } from "../format/display";
import { useUi } from "../app/ui";
import { Avatar } from "../screens/contacts/Avatar";
import {
  contactAside,
  contactLetters,
  dayTitle,
  detailDate,
  groupAmount,
  groupByDay,
  groupChip,
  groupConsecutiveMixing,
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
  type MixingGroup,
} from "../format/tx";
import { t as tNow, useT } from "../i18n";
import { AmountText, Chip, Icon, VisuallyHidden } from "./basics";
import { EmptyArt, type EmptyArtId } from "./emptyArt";
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
  /** Each amount's value now (rates-engine §2). */
  fiat: Fiat;
  /** Discreet mode: no fiat value shows. */
  discreet: boolean;
}

/** Display unit, locale, clock and price the rows of a screen format with. */
export function useRowContext(network: NetworkInfo): RowContext {
  const backend = useBackend();
  const fiat = useFiat();
  const discreet = useUi((s) => s.discreet);
  return { display: useDisplay(network), locale: backend.locale, now: backend.now(), fiat, discreet };
}

/** Test id of a row: stable across engines and runs for the same transaction record. */
export const rowTestId = (row: TxRow) => `tx-row-${row.txid.slice(0, 10)}-${row.recordIndex}`;

/** What a row shows beyond its record (U-03 D14). Counterparty attribution (DP3-02) names the
 * contact through `txTitle`, and a named contact's avatar leads the row (DP3-07 D1). */
export interface RowDecor {
  title?: string;
  leading?: ReactNode;
  /** The row's source, as a tag beside the time (Platform rows among the Core ones). */
  tag?: string;
  /** Replaces the status chip with a neutral note (not a problem). */
  note?: string;
  /** The row shows no amount (it has none to show). */
  hideAmount?: boolean;
  /** Who the row is with, shown under its title (a contact paid from a balance). */
  party?: string;
}

function TxRowView({ row, ctx, onOpen, decorate }: { row: TxRow; ctx: RowContext; onOpen: (row: TxRow) => void; decorate?: (row: TxRow) => RowDecor }) {
  const decor = decorate?.(row);
  const title = decor?.title ?? txTitle(row);
  const chip = decor?.note ?? txChip(row.status);
  const trailing = trailingStatus(row.status);
  const date = rowDate(row);
  const shown = shownAmount(row.amount, row.txType);
  const amount = ctx.display.compact(shown.duffs, shown.signed);
  const letters = contactLetters(row.counterparty);
  const aside = decor?.party ? null : contactAside(row, title);
  const leading =
    decor?.leading ??
    (letters !== null ? <Avatar letters={letters} identity={row.counterparty?.identity} /> : <Icon id={TX_ICON[txIcon(row.txType, row.status, row.amount)]} size={30} className="tx-row-icon" />);
  const body = (
    <>
      {leading}
      <span className="tx-row-text">
        <span className={`tx-row-title ${isProblem(row.status) ? "is-problem" : ""}`}>{title}</span>
        {aside && <VisuallyHidden>{aside}</VisuallyHidden>}
        <span className="tx-row-subtitle">
          {decor?.party && <span className="tx-row-party">{decor.party}</span>}
          {date && <span className="tx-row-time">{timeOfDay(date, ctx.locale)}</span>}
          {decor?.tag && <Chip tone="info">{decor.tag}</Chip>}
          {chip && <Chip tone={isProblem(row.status) ? "danger" : "info"}>{chip}</Chip>}
        </span>
      </span>
      <span className="tx-row-trailing">
        {trailing && <span className="tx-row-status">{trailing}</span>}
        {!decor?.hideAmount && <AmountText className="tx-row-amount" text={amount} unitName={ctx.display.name} />}
        {!decor?.hideAmount && <FiatLine fiat={ctx.fiat} duffs={shown.duffs} hidden={ctx.discreet} className="tx-row-fiat" testId={`${rowTestId(row)}-fiat`} />}
      </span>
    </>
  );
  return (
    <li className="tx-row-item">
      <button type="button" className="tx-row" data-testid={rowTestId(row)} data-txid={row.txid} onClick={() => onOpen(row)}>
        {body}
      </button>
    </li>
  );
}

function MixingGroupView({ group, ctx, onOpen, decorate }: { group: MixingGroup; ctx: RowContext; onOpen: (row: TxRow) => void; decorate?: (row: TxRow) => RowDecor }) {
  const t = useT();
  const [expanded, setExpanded] = useState(false);
  const membersId = useId();
  const first = group.rows[0];
  const chip = groupChip(group.rows);
  const amount = groupAmount(group.rows);
  const shown = amount === null ? null : shownAmount(amount, "coinJoinMixing");
  const testId = `tx-mixing-group-${first.txid.slice(0, 10)}-${first.recordIndex}`;
  const date = rowDate(first);
  return (
    <>
      <li className="tx-row-item">
        <button
          type="button"
          className="tx-row tx-mixing-group"
          data-testid={testId}
          aria-expanded={expanded}
          aria-controls={expanded ? membersId : undefined}
          onClick={() => setExpanded((value) => !value)}
        >
          <Icon id={TX_ICON.mixing} size={30} className="tx-row-icon" />
          <span className="tx-row-text">
            <span className="tx-row-title">{t("components.history.mixingGroup", { count: group.rows.length })}</span>
            <span className="tx-row-subtitle">
              {date && <span className="tx-row-time">{timeOfDay(date, ctx.locale)}</span>}
              {chip && <Chip tone="info">{chip}</Chip>}
            </span>
          </span>
          {shown && (
            <span className="tx-row-trailing">
              <AmountText className="tx-row-amount" text={ctx.display.compact(shown.duffs, shown.signed)} unitName={ctx.display.name} />
              <FiatLine fiat={ctx.fiat} duffs={shown.duffs} hidden={ctx.discreet} className="tx-row-fiat" testId={`${testId}-fiat`} />
            </span>
          )}
        </button>
      </li>
      {expanded && (
        <li className="tx-mixing-members">
          <ul id={membersId} className="tx-mixing-members-list" role="list">
            {group.rows.map((row) => (
              <TxRowView key={`${row.txid}:${row.recordIndex}`} row={row} ctx={ctx} onOpen={onOpen} decorate={decorate} />
            ))}
          </ul>
        </li>
      )}
    </>
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
  const t = useT();
  const titleId = useId();
  const title = day ? dayTitle(day, ctx.now, ctx.locale) : t("components.history.undated");
  const grouped = groupConsecutiveMixing(rows);
  return (
    <div className="tx-group card-l1" role="group" aria-labelledby={titleId} data-testid={testId}>
      <header className="tx-group-header">
        <h3 className="tx-group-day" id={titleId}>
          {title}
        </h3>
        {day && <span className="tx-group-weekday">{weekday(day, ctx.locale)}</span>}
      </header>
      <ul className="tx-group-rows" role="list">
        {grouped.map((item) =>
          "kind" in item ? (
            <MixingGroupView key={`mixing:${item.rows[item.rows.length - 1].txid}:${item.rows[item.rows.length - 1].recordIndex}`} group={item} ctx={ctx} onOpen={onOpen} decorate={decorate} />
          ) : (
            <TxRowView key={`${item.txid}:${item.recordIndex}`} row={item} ctx={ctx} onOpen={onOpen} decorate={decorate} />
          ),
        )}
      </ul>
    </div>
  );
}

/** Table mode's status text (dash-qt's status column tooltip, shortened). */
export function tableStatus(status: TxRow["status"]): string {
  const chip = txChip(status);
  if (chip) return chip;
  if (status.kind === "immature") return tNow("components.history.immature", { blocks: status.maturesIn ?? "?" });
  if (status.chainLocked) return tNow("components.history.chainLocked");
  return tNow("components.history.confirmations", { count: status.confirmations });
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
  const t = useT();
  const value = ctx.fiat.shown && !ctx.discreet ? ctx.fiat.view?.rate?.currency : undefined;
  return (
    <div className="card-l1 table-card" data-testid="tx-table">
      <table className="data-table tx-table">
        <caption className="sr-only">{t("components.history.caption")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("components.history.status")}</th>
            <SortHeader column="date" title={t("components.history.date")} sort={sort} onSort={onSort} />
            <th scope="col">{t("components.history.type")}</th>
            <th scope="col">{t("components.history.party")}</th>
            <SortHeader column="amount" title={t("components.history.amount", { unit: ctx.display.name })} sort={sort} onSort={onSort} className="num" />
            {value && (
              <th scope="col" className="num" data-testid="tx-table-value">
                {t("components.history.value", { currency: value })}
              </th>
            )}
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
                    {date ? detailDate(date, ctx.locale) : t("components.history.undated")}
                  </button>
                </td>
                <td>{tableTypeTitle(row.txType)}</td>
                <td className="tx-table-party" title={row.address ?? undefined}>
                  {rowParty(row)}
                </td>
                <td className="num">
                  <bdi dir="ltr">{ctx.display.full(shown.duffs, shown.signed)}</bdi>
                </td>
                {value && (
                  <td className={`num${ctx.fiat.stale ? " is-stale" : ""}`} title={ctx.fiat.staleTitle} data-testid={`table-${rowTestId(row)}-fiat`}>
                    {ctx.fiat.text(shown.duffs)}
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** Move focus after a control that unmounts on this click, once the next frame has removed it. */
export function focusWhenIdle(el: { readonly current: { focus(): void } | null }) {
  requestAnimationFrame(() => el.current?.focus());
}

export function EmptyState({ title, message, action, testId, art, plain }: { title: string; message?: string; action?: ReactNode; testId: string; art?: EmptyArtId; /** Inside a card already: no second card chrome. */ plain?: boolean }) {
  return (
    <div className={plain ? "empty-state" : "empty-state card-l1"} data-testid={testId} data-art={art}>
      {art ? <EmptyArt id={art} /> : <Icon id="txAll" size={60} className="empty-state-icon" />}
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
