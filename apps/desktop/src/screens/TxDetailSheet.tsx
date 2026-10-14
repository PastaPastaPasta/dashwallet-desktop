// Transaction detail sheet (UX-SPEC §4.9, C23 medium): iOS TxDetail header with the dash-qt
// facts (research 02 §4.6), the label editor, and Abandon / Resend where they apply (U-03 D15).

import { useId, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { TxDetails } from "../api/generated/TxDetails";
import type { TxStatusInfo } from "../api/generated/TxStatusInfo";
import { keys, useBackend, useTxDetail, useTxDetailExtras } from "../api/queries";
import { ui } from "../app/ui";
import { AmountText, Chip, DashButton, Icon } from "../components/basics";
import type { UsedIcon } from "../components/icons";
import { LoadingState } from "../components/history";
import { Sheet } from "../components/sheet";
import { CopyValue, DetailRow } from "../components/detail";
import { full } from "../format/amount";
import { contactName, detailDate, isProblem, rowDate, shownAmount, txChip, txIcon, txTitle } from "../format/tx";

/** dash-qt `TransactionDesc::FormatTxStatus`, for the states the engine reports. */
function statusText(s: TxStatusInfo): string {
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

/** The transaction's label (QT-093): saved on Save, cleared when empty. */
function LabelEditor({ walletId, txid, label }: { walletId: string; txid: string; label: string | null }) {
  const b = useBackend();
  const client = useQueryClient();
  const id = useId();
  const [text, setText] = useState(label ?? "");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const changed = text.trim() !== (label ?? "");
  const save = async () => {
    setBusy(true);
    setProblem(null);
    try {
      await b.setTxLabel(walletId, txid, text.trim() || null);
      await client.invalidateQueries({ queryKey: keys.historyAll(walletId) });
      await client.invalidateQueries({ queryKey: keys.txDetail(walletId, txid) });
    } catch (e) {
      setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="label-editor card-l1"
      data-testid="tx-detail-label"
      onSubmit={(e) => {
        e.preventDefault();
        if (changed && !busy) void save();
      }}
    >
      <label htmlFor={id}>Label</label>
      <span className="label-editor-row">
        <input id={id} data-testid="tx-label-input" value={text} autoComplete="off" placeholder="Add a label" onChange={(e) => setText(e.target.value)} />
        <DashButton type="submit" variant="tintedBlue" size="small" data-testid="tx-label-save" disabled={!changed || busy}>
          Save
        </DashButton>
      </span>
      {problem && (
        <span className="field-error" role="alert">
          {problem}
        </span>
      )}
    </form>
  );
}

/** Abandon and Resend (QT-091), shown only when the engine says they apply. */
function Actions({ walletId, txid }: { walletId: string; txid: string }) {
  const b = useBackend();
  const client = useQueryClient();
  const extras = useTxDetailExtras(walletId, txid);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<{ text: string; problem: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const x = extras.data;
  const can = !!x && (x.canAbandon || x.canResend);
  // After an abandon the actions go, but its note stays.
  if (!x || (!can && !note)) return null;
  const run = async (what: "abandon" | "resend") => {
    setBusy(true);
    setNote(null);
    try {
      if (what === "abandon") await b.abandonTransaction(walletId, txid);
      else await b.resendTransaction(walletId, txid);
      setNote({ text: what === "abandon" ? "The transaction was abandoned." : "The transaction was sent to the network again.", problem: false });
      setConfirming(false);
      await client.invalidateQueries({ queryKey: keys.txDetailAll(walletId) });
      await client.invalidateQueries({ queryKey: keys.historyAll(walletId) });
    } catch (e) {
      setNote({ text: errorMessage(e), problem: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="tx-actions card-l1" aria-label="Actions" data-testid="tx-detail-actions">
      {confirming ? (
        <>
          <p className="tx-actions-question">
            Abandon this transaction? Its coins come back to the balance; if it reaches the network anyway, it still counts.
          </p>
          <span className="tx-actions-row">
            <DashButton variant="tintedGray" size="small" disabled={busy} onClick={() => setConfirming(false)}>
              Keep it
            </DashButton>
            <DashButton variant="plainRed" size="small" data-testid="tx-abandon-confirm" disabled={busy} onClick={() => void run("abandon")}>
              Abandon transaction
            </DashButton>
          </span>
        </>
      ) : (
        can && (
          <span className="tx-actions-row">
            {x.canResend && (
              <DashButton variant="tintedBlue" size="small" data-testid="tx-resend" disabled={busy} onClick={() => void run("resend")}>
                Resend
              </DashButton>
            )}
            {x.canAbandon && (
              <DashButton variant="plainRed" size="small" data-testid="tx-abandon" disabled={busy} onClick={() => setConfirming(true)}>
                Abandon transaction
              </DashButton>
            )}
          </span>
        )
      )}
      {note && (
        <p className={note.problem ? "form-problem" : "placeholder-note"} role={note.problem ? "alert" : "status"} data-testid="tx-action-note">
          {note.text}
        </p>
      )}
    </section>
  );
}

function Body({ d, network, walletId }: { d: TxDetails; network: NetworkInfo; walletId: string }) {
  const backend = useBackend();
  const row = d.rows[0];
  const amount = d.rows.reduce((sum, r) => sum + r.amount, 0);
  const icon = row ? txIcon(row.txType, d.status, amount) : "received";
  const headerIcon: UsedIcon = icon === "error" ? "txDetailError" : amount < 0 ? "txDetailSent" : "txDetailReceived";
  const chip = txChip(d.status);
  const date = rowDate(d);
  // The row's address; else, received: our own output, sent: the first output that is not ours.
  const counterparty =
    row?.address ?? d.outputs.find((o) => (amount >= 0 ? o.isMine : !o.isMine) && !o.isChange)?.address ?? null;
  const shown = shownAmount(amount, row?.txType ?? "other");
  const own = d.outputs.some((o) => o.isMine && o.address === counterparty);
  // The DashPay contact the payment went to or came from (DP3-02); one per transaction.
  const contact = contactName(d.rows.find((r) => r.counterparty)?.counterparty);
  const unit = network.unitName;
  return (
    <>
      <div className="tx-detail-summary" data-testid="tx-detail-summary">
        <Icon id={headerIcon} size={50} />
        <h3 className="tx-detail-title">{row ? txTitle({ ...row, amount }) : "Transaction"}</h3>
        <AmountText className="tx-detail-amount" text={full(shown.duffs, shown.signed)} unitName={unit} glyphFactor={0.8} />
        <div className="tx-detail-status">
          {chip && <Chip tone={isProblem(d.status) ? "danger" : "info"}>{chip}</Chip>}
          {date && <span className="tx-detail-date">{detailDate(date, backend.locale)}</span>}
        </div>
      </div>
      <dl className="detail-list card-l1" data-testid="tx-detail-facts">
        {contact && (
          <DetailRow label={amount < 0 ? "Sent to contact" : "Received from contact"} testId="tx-detail-contact">
            {contact}
          </DetailRow>
        )}
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
        {d.message && <DetailRow label="Message">{d.message}</DetailRow>}
      </dl>
      <LabelEditor key={`label-${d.txid}`} walletId={walletId} txid={d.txid} label={d.label} />
      <Actions key={`actions-${d.txid}`} walletId={walletId} txid={d.txid} />
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
        <Body d={detail.data} network={network} walletId={walletId} />
      ) : detail.isError ? (
        <p role="alert">{errorMessage(detail.error)}</p>
      ) : (
        <LoadingState testId="tx-detail-loading" text="Loading transaction" />
      )}
    </Sheet>
  );
}
