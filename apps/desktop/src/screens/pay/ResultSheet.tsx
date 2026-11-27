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
import { messageTable, useT } from "../../i18n";
import { broadcastFailure, type Outcome, type Sending } from "./model";
import type { DraftApi } from "./useDraft";

const TITLE: Readonly<Record<Outcome["kind"], string>> = messageTable("pay.result.title", ["sent", "unknown", "failed"]);

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
  retrying,
  allowRetry = true,
  onRetry,
  onDone,
}: {
  outcome: Outcome;
  network: NetworkInfo;
  draft: DraftApi;
  returnFocus: () => HTMLElement | null;
  /** A Try again is running, perhaps from before a lock. */
  retrying: boolean;
  allowRetry?: boolean;
  /** Starts a Try again: where its outcome goes, or `null` when there is none to run. */
  onRetry: () => ((o: Outcome) => void) | null;
  /** `show`: open the transaction in Activity. */
  onDone: (show: boolean) => void;
}) {
  const t = useT();
  const [running, setRunning] = useState(false);
  const busy = running || retrying;
  const canRetry = outcome.kind === "unknown" && outcome.retry && allowRetry;
  // Finished retries, for a harness that must not miss a fast one's busy state (DP3-06 Sol r1 #1).
  const [tries, setTries] = useState(0);
  const shown = useDisplay(network);
  const unit = shown.name;

  const retryButton = useRef<HTMLButtonElement>(null);
  const retry = async () => {
    if (!canRetry || busy) return;
    const settle = onRetry();
    if (!settle) return;
    if (retryButton.current === document.activeElement) retryButton.current?.closest<HTMLElement>('[role="dialog"]')?.focus();
    setRunning(true);
    const { txid: held, draft: d, prepared, amount, fee, note } = outcome;
    try {
      const { txid } = await draft.broadcast(d, prepared);
      settle({ kind: "sent", txid, amount, fee, note });
    } catch (e) {
      settle(broadcastFailure(e, shown.chain, { txid: held, draft: d, prepared, amount, fee, note }, false, shown.amountUnit));
    } finally {
      setRunning(false);
      setTries((n) => n + 1);
    }
  };
  // The button was disabled while the retry ran: focus goes back to it, or to Close, unless the
  // user moved it meanwhile.
  const closeButton = useRef<HTMLButtonElement>(null);
  const heldAt = useRef<Element | null>(null);
  useEffect(() => {
    if (busy) {
      heldAt.current ??= document.activeElement;
      return;
    }
    const held = heldAt.current;
    heldAt.current = null;
    if (outcome.kind !== "unknown") return;
    const a = document.activeElement;
    if (a !== held && a !== document.body && a !== closeButton.current?.closest('[role="dialog"]')) return;
    (canRetry ? retryButton : closeButton).current?.focus();
  }, [busy, canRetry, outcome]);

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
                {t("pay.result.unknownText", { text: outcome.text })}
              </p>
            )}
          </>
        )}
      </div>
      {outcome.kind !== "failed" && (
        <dl className="detail-list card-l1" data-testid="pay-result-facts">
          <DetailRow label={t("pay.result.networkFee")}>
            <AmountText text={shown.full(outcome.fee)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label={t("pay.result.txid")} testId="pay-result-txid">
            <CopyValue value={outcome.txid} action={t("pay.result.copyTxid")} mono />
          </DetailRow>
        </dl>
      )}
      <div className="sheet-actions">
        {outcome.kind === "sent" && (
          <>
            <DashButton variant="tintedBlue" size="large" data-testid="pay-result-show" onClick={() => onDone(true)}>
              {t("pay.result.show")}
            </DashButton>
            <DashButton variant="filledBlue" size="large" data-testid="pay-result-close" data-autofocus onClick={() => onDone(false)}>
              {t("pay.result.close")}
            </DashButton>
          </>
        )}
        {outcome.kind === "unknown" && (
          <>
            <DashButton ref={closeButton} variant="tintedGray" size="large" data-testid="pay-result-close" data-autofocus={!canRetry || undefined} disabled={busy} onClick={() => onDone(false)}>
              {t("pay.result.close")}
            </DashButton>
            {(canRetry || busy) && (
              <DashButton ref={retryButton} variant="filledBlue" size="large" data-testid="pay-result-retry" data-autofocus disabled={busy} onClick={() => void retry()}>
                {busy ? t("pay.result.sending") : t("pay.result.retry")}
              </DashButton>
            )}
          </>
        )}
        {outcome.kind === "failed" && (
          <DashButton variant="filledBlue" size="large" data-testid="pay-result-back" data-autofocus onClick={() => onDone(false)}>
            {t("pay.result.back")}
          </DashButton>
        )}
      </div>
    </Sheet>
  );
}

/** A payment whose broadcast is still running when its form comes back (after an unlock): held up
 * until the result replaces it, so nothing behind it looks unsent. */
export function SendingSheet({ sending, network }: { sending: Sending; network: NetworkInfo }) {
  const t = useT();
  const shown = useDisplay(network);
  return (
    <Sheet title={t("pay.result.sendingTitle")} width="small" testId="pay-sending" closable={false} onClose={() => undefined}>
      <div className="result" role="status">
        <span className="spinner" aria-hidden="true" />
        <AmountText className="result-amount" text={shown.full(sending.amount)} unitName={shown.name} glyphFactor={0.8} />
        <p className="result-text">{t("pay.result.sendingText")}</p>
      </div>
    </Sheet>
  );
}
