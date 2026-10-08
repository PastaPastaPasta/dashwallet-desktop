// Transaction detail sheet (UX-SPEC §4.9, C23 medium): iOS TxDetail header with the dash-qt
// facts (research 02 §4.6). Abandon/Resend and the label editor are not part of the spike.

import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxDetails } from "../api/generated/TxDetails";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { useBackend, useTxDetail } from "../api/queries";
import { ui } from "../app/ui";
import { AmountText, Chip, Icon } from "../components/basics";
import type { UsedIcon } from "../components/icons";
import { LoadingState } from "../components/history";
import { Sheet } from "../components/sheet";
import { copyWithToast } from "../components/toast";
import { full } from "../format/amount";
import { chipIsProblem, detailDate, isInternal, rowDate, txChip, txIcon, txTitle } from "../format/tx";

/** dash-qt `TransactionDesc::FormatTxStatus`, for the states the engine reports. */
export function statusText(s: TxStatusInfo): string {
  switch (s.kind) {
    case "abandoned":
      return "Abandoned";
    case "conflicted":
      return "Conflicted";
    case "notAccepted":
      return "Not accepted";
    case "unconfirmed":
      return s.instantLocked ? "0/unconfirmed, verified via InstantSend" : "0/unconfirmed, in memory pool";
    case "immature":
      return `${s.confirmations} confirmations, matures in ${s.maturesIn ?? "?"} more blocks`;
    case "confirming":
    case "confirmed": {
      const base = s.kind === "confirming" ? `${s.confirmations}/unconfirmed` : `${s.confirmations} confirmations`;
      return s.chainLocked ? `${base}, locked via ChainLocks` : s.instantLocked ? `${base}, verified via InstantSend` : base;
    }
  }
}

function DetailRow({ label, children, testId }: { label: string; children: React.ReactNode; testId?: string }) {
  return (
    <div className="detail-row" data-testid={testId}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value">{children}</dd>
    </div>
  );
}

function CopyValue({ value, label, mono = false }: { value: string; label: string; mono?: boolean }) {
  return (
    <span className="copy-value">
      <span className={mono ? "mono" : undefined}>{value}</span>
      <button type="button" className="copy-button" aria-label={`Copy ${label}`} title={`Copy ${label}`} onClick={() => void copyWithToast(value)}>
        <Icon id="copy" size={20} />
      </button>
    </span>
  );
}

function Body({ d, network }: { d: TxDetails; network: NetworkInfo }) {
  const backend = useBackend();
  const row = d.rows[0];
  const amount = d.rows.reduce((sum, r) => sum + r.amount, 0);
  const icon = row ? txIcon(row.txType, d.status, amount) : "received";
  const headerIcon: UsedIcon = icon === "error" ? "txDetailError" : amount < 0 ? "txDetailSent" : "txDetailReceived";
  const chip = txChip(d.status);
  const date = rowDate(d);
  const counterparty = row?.address ?? d.outputs.find((o) => !o.isChange)?.address ?? null;
  const own = d.outputs.some((o) => o.isMine && o.address === counterparty);
  const unit = network.unitName;
  return (
    <>
      <div className="tx-detail-summary" data-testid="tx-detail-summary">
        <Icon id={headerIcon} size={50} />
        <h3 className="tx-detail-title">{row ? txTitle({ ...row, amount }) : "Transaction"}</h3>
        <AmountText className="tx-detail-amount" text={full(amount, !(row && isInternal(row.txType)))} unitName={unit} glyphFactor={0.8} />
        <div className="tx-detail-status">
          {chip && <Chip tone={chipIsProblem(d.status) ? "danger" : "info"}>{chip}</Chip>}
          {date && <span className="tx-detail-date">{detailDate(date, backend.locale)}</span>}
        </div>
      </div>
      <dl className="detail-list card-l1" data-testid="tx-detail-facts">
        {counterparty && (
          <DetailRow label={amount < 0 ? "To" : "Received with"}>
            <CopyValue value={counterparty} label="address" />
            {own && <span className="detail-note"> (own address)</span>}
          </DetailRow>
        )}
        {d.fee !== null && (
          <DetailRow label="Network fee">
            <AmountText text={full(d.fee, false)} unitName={unit} glyph={false} />
          </DetailRow>
        )}
        <DetailRow label="Status">{statusText(d.status)}</DetailRow>
        {d.blockHeight !== null && <DetailRow label="Block">{d.blockHeight.toLocaleString(backend.locale)}</DetailRow>}
        <DetailRow label="Transaction ID" testId="tx-detail-txid">
          <CopyValue value={d.txid} label="transaction ID" mono />
        </DetailRow>
        <DetailRow label="Size">{`${d.sizeBytes} bytes`}</DetailRow>
        {d.label && <DetailRow label="Label">{d.label}</DetailRow>}
        {d.message && <DetailRow label="Message">{d.message}</DetailRow>}
      </dl>
      <details className="disclosure card-l1" data-testid="tx-detail-inputs">
        <summary>Inputs ({d.inputs.length})</summary>
        <ul role="list" className="io-list">
          {d.inputs.map((i) => (
            <li key={`${i.previousTxid}:${i.previousVout}`} className="io-row">
              <span className="mono io-ref">{`${i.previousTxid.slice(0, 16)}…:${i.previousVout}`}</span>
              <span>{i.address ?? "—"}</span>
              {i.amount !== null ? <AmountText text={full(i.amount, false)} unitName={unit} glyph={false} /> : <span title="Not a wallet input">—</span>}
            </li>
          ))}
        </ul>
      </details>
      <details className="disclosure card-l1" data-testid="tx-detail-outputs">
        <summary>Outputs ({d.outputs.length})</summary>
        <ul role="list" className="io-list">
          {d.outputs.map((o) => (
            <li key={o.vout} className="io-row">
              <span className="mono io-ref">#{o.vout}</span>
              <span>
                {o.address ?? "—"}
                {o.isChange ? " (change)" : o.isMine ? " (own address)" : ""}
              </span>
              <AmountText text={full(o.amount, false)} unitName={unit} glyph={false} />
            </li>
          ))}
        </ul>
      </details>
    </>
  );
}

export function TxDetailSheet({ walletId, txid, network }: { walletId: string; txid: string; network: NetworkInfo }) {
  const detail = useTxDetail(walletId, txid);
  return (
    <Sheet title="Transaction details" width="medium" testId="tx-detail" onClose={() => ui.set({ detail: null })}>
      {detail.data ? (
        <Body d={detail.data} network={network} />
      ) : detail.isError ? (
        <p role="alert">{String((detail.error as { message?: string }).message ?? detail.error)}</p>
      ) : (
        <LoadingState testId="tx-detail-loading" text="Loading transaction" />
      )}
    </Sheet>
  );
}
