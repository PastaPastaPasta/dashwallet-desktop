// The payment's result (UX-SPEC §4.7, U-03 D10): sent, outcome unknown (the engine holds the
// prepared transaction, so Try again repeats the broadcast), or not sent. Once unknown, a
// payment never becomes "not sent": a retry's failure stays unknown.

import { useEffect, useRef, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { AmountText, DashButton, Icon } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import type { UsedIcon } from "../../components/icons";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { broadcastFailure, type Outcome } from "./model";
import type { DraftApi } from "./useDraft";

const TITLE: Record<Outcome["kind"], string> = {
  sent: "Payment sent",
  unknown: "Payment status unknown",
  failed: "Payment not sent",
};

const ICON: Record<Outcome["kind"], UsedIcon> = {
  sent: "txDetailSent",
  unknown: "messageWarning",
  failed: "txDetailError",
};

export function ResultSheet({
  outcome,
  network,
  draft,
  returnFocus,
  onOutcome,
  onDone,
}: {
  outcome: Outcome;
  network: NetworkInfo;
  draft: DraftApi;
  returnFocus: () => HTMLElement | null;
  onOutcome: (o: Outcome) => void;
  /** `show`: open the transaction in Activity. */
  onDone: (show: boolean) => void;
}) {
  const [busy, setBusy] = useState(false);
  // Finished retries, for a harness that must not miss a fast one's busy state (DP3-06 Sol r1 #1).
  const [tries, setTries] = useState(0);
  const shown = useDisplay(network);
  const unit = shown.name;

  const retryButton = useRef<HTMLButtonElement>(null);
  const retry = async () => {
    if (outcome.kind !== "unknown" || !outcome.retry || busy) return;
    setBusy(true);
    const { draft: d, prepared, amount, fee, note } = outcome;
    try {
      const { txid } = await draft.broadcast(d, prepared);
      onOutcome({ kind: "sent", txid, amount, fee, note });
    } catch (e) {
      onOutcome(broadcastFailure(e, shown.chain, { draft: d, prepared, amount, fee, note }, false, shown.amountUnit));
    } finally {
      setBusy(false);
      setTries((n) => n + 1);
    }
  };
  // The button was disabled while the retry ran: focus goes back to it, or to Close.
  const closeButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!busy && outcome.kind === "unknown") (outcome.retry ? retryButton : closeButton).current?.focus();
  }, [busy, outcome]);

  return (
    <Sheet title={TITLE[outcome.kind]} width="small" testId="pay-result" returnFocus={returnFocus} onClose={() => !busy && onDone(false)}>
      <div className={`result result-${outcome.kind}`} data-testid={`pay-result-${outcome.kind}`} data-retries={tries}>
        <Icon id={ICON[outcome.kind]} size={50} />
        {outcome.kind === "failed" ? (
          <p className="result-text" role="alert" data-testid="pay-result-text">
            {outcome.text}
          </p>
        ) : (
          <>
            <AmountText className="result-amount" text={shown.full(outcome.amount)} unitName={unit} glyphFactor={0.8} />
            {outcome.kind === "sent" && outcome.note && (
              <p className="result-text" data-testid="pay-result-note">
                {outcome.note}
              </p>
            )}
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
            <AmountText text={shown.full(outcome.fee)} unitName={unit} glyph={false} />
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
            <DashButton ref={closeButton} variant="tintedGray" size="large" data-testid="pay-result-close" disabled={busy} onClick={() => onDone(false)}>
              Close
            </DashButton>
            {outcome.retry && (
              <DashButton ref={retryButton} variant="filledBlue" size="large" data-testid="pay-result-retry" data-autofocus disabled={busy} onClick={() => void retry()}>
                {busy ? "Sending…" : "Try again"}
              </DashButton>
            )}
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
