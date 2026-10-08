// History components (UX-SPEC C7, C8, C9, C25, C26): day-group cards of iOS transaction rows.

import type { ReactNode } from "react";
import type { TxRow } from "../api/generated/TxRow";
import { compact } from "../format/amount";
import {
  chipIsProblem,
  dayTitle,
  groupByDay,
  isInternal,
  rowDate,
  timeOfDay,
  trailingStatus,
  txChip,
  txIcon,
  txTitle,
  weekday,
  type TxIcon,
} from "../format/tx";
import { AmountText, Chip, Icon } from "./basics";
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
  unitName: string;
  locale: string | undefined;
  now: Date;
}

/** Test id of a row: stable across engines and runs for the same transaction record. */
export const rowTestId = (row: TxRow) => `tx-row-${row.txid.slice(0, 10)}-${row.recordIndex}`;

export function TxRowView({ row, ctx, onOpen }: { row: TxRow; ctx: RowContext; onOpen: (row: TxRow) => void }) {
  const title = txTitle(row);
  const chip = txChip(row.status);
  const trailing = trailingStatus(row.status);
  const date = rowDate(row);
  const amount = compact(row.amount, !isInternal(row.txType));
  return (
    <li className="tx-row-item">
      <button type="button" className="tx-row" data-testid={rowTestId(row)} data-txid={row.txid} onClick={() => onOpen(row)}>
        <Icon id={TX_ICON[txIcon(row.txType, row.status, row.amount)]} size={30} className="tx-row-icon" />
        <span className="tx-row-text">
          <span className={`tx-row-title ${chipIsProblem(row.status) ? "is-problem" : ""}`}>{title}</span>
          <span className="tx-row-subtitle">
            {date && <span className="tx-row-time">{timeOfDay(date, ctx.locale)}</span>}
            {chip && <Chip tone={chipIsProblem(row.status) ? "danger" : "info"}>{chip}</Chip>}
          </span>
        </span>
        <span className="tx-row-trailing">
          {trailing && <span className="tx-row-status">{trailing}</span>}
          <AmountText className="tx-row-amount" text={amount} unitName={ctx.unitName} />
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
}: {
  rows: TxRow[];
  ctx: RowContext;
  onOpen: (row: TxRow) => void;
  testIdPrefix: string;
}) {
  const groups = groupByDay(rows, rowDate);
  return (
    <div className="tx-groups">
      {groups.map((g, i) => {
        const title = g.day ? dayTitle(g.day, ctx.now, ctx.locale) : "Undated";
        return (
          <section key={g.day?.getTime() ?? "undated"} className="tx-group card-l1" data-testid={`${testIdPrefix}-group-${i}`} aria-label={title}>
            <header className="tx-group-header">
              <h3 className="tx-group-day">{title}</h3>
              {g.day && <span className="tx-group-weekday">{weekday(g.day, ctx.locale)}</span>}
            </header>
            <ul className="tx-group-rows" role="list">
              {g.items.map((row) => (
                <TxRowView key={`${row.txid}:${row.recordIndex}`} row={row} ctx={ctx} onOpen={onOpen} />
              ))}
            </ul>
          </section>
        );
      })}
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
