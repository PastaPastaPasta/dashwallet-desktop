// The payment's result (UX-SPEC §4.7, U-03 D10): sent, outcome unknown (the prepared
// transaction is held, so Try again repeats the broadcast), or not sent.

import { useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import { full } from "../../format/amount";
import { broadcastFailure, type Outcome } from "./model";
import { chainOf } from "./PayToAddress";
import type { DraftApi } from "./useDraft";

const TITLE: Record<Outcome["kind"], string> = {
  sent: "Payment sent",
  unknown: "Payment status unknown",
  failed: "Payment not sent",
};

export function ResultSheet({
  outcome,
  network,
  draft,
  onOutcome,
  onDone,
}: {
  outcome: Outcome;
  network: NetworkInfo;
  draft: DraftApi;
  onOutcome: (o: Outcome) => void;
  /** `show`: open the transaction in Activity. */
  onDone: (show: boolean) => void;
}) {
  const [busy, setBusy] = useState(false);
  const unit = network.unitName;

  const retry = async () => {
    if (outcome.kind !== "unknown" || busy) return;
    setBusy(true);
    const { draft: d, prepared, amount, fee } = outcome;
    try {
      const { txid } = await draft.broadcast(d, prepared);
      onOutcome({ kind: "sent", txid, amount, fee });
    } catch (e) {
      const next = broadcastFailure(e, chainOf(network), { draft: d, prepared, amount, fee });
      if (next.kind === "failed") await draft.abandon(d, prepared);
      onOutcome(next);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet title={TITLE[outcome.kind]} width="small" testId="pay-result" onClose={() => !busy && onDone(false)}>
      <div className={`result result-${outcome.kind}`} data-testid={`pay-result-${outcome.kind}`}>
        <Icon id={outcome.kind === "sent" ? "txDetailSent" : outcome.kind === "unknown" ? "messageWarning" : "txDetailError"} size={50} />
        {outcome.kind === "failed" ? (
          <p className="result-text" role="alert" data-testid="pay-result-text">
            {outcome.text}
          </p>
        ) : (
          <>
            <AmountText className="result-amount" text={full(outcome.amount, false)} unitName={unit} glyphFactor={0.8} />
            {outcome.kind === "unknown" && (
              <p className="result-text" role="alert" data-testid="pay-result-text">
                {outcome.text} Check Activity before you pay again.
              </p>
            )}
          </>
        )}
      </div>
      {outcome.kind === "sent" && (
        <dl className="detail-list card-l1" data-testid="pay-result-facts">
          <DetailRow label="Network fee">
            <AmountText text={full(outcome.fee, false)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label="Transaction ID" testId="pay-result-txid">
            <CopyValue value={outcome.txid} label="transaction ID" mono />
          </DetailRow>
        </dl>
      )}
      <div className="sheet-actions">
        {outcome.kind === "sent" && (
          <>
            <DashButton variant="tintedBlue" size="large" data-testid="pay-result-show" onClick={() => onDone(true)}>
              Show in Activity
            </DashButton>
            <DashButton variant="filledBlue" size="large" data-testid="pay-result-close" data-autofocus onClick={() => onDone(false)}>
              Close
            </DashButton>
          </>
        )}
        {outcome.kind === "unknown" && (
          <>
            <DashButton variant="tintedGray" size="large" data-testid="pay-result-close" disabled={busy} onClick={() => onDone(false)}>
              Close
            </DashButton>
            <DashButton variant="filledBlue" size="large" data-testid="pay-result-retry" data-autofocus disabled={busy} onClick={() => void retry()}>
              {busy ? "Sending…" : "Try again"}
            </DashButton>
          </>
        )}
        {outcome.kind === "failed" && (
          <DashButton variant="filledBlue" size="large" data-testid="pay-result-back" data-autofocus onClick={() => onDone(false)}>
            Back to payment
          </DashButton>
        )}
      </div>
    </Sheet>
  );
}
