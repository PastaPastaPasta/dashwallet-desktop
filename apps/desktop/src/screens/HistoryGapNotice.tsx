import { useId } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { keys, useBackend, useHistoryGap, useWallets } from "../api/queries";
import { DashButton } from "../components/basics";
import { Sheet } from "../components/sheet";
import { useT } from "../i18n";
import { useModel } from "../onboarding/model";
import { historyGapModel, HistoryGapText } from "../tools/historyGap";
import { RepairText } from "../tools/repair";

/** Says which open wallets' history starts before the stored block data, and offers Repair's reset. */
export function HistoryGapNotice() {
  const tr = useT();
  const b = useBackend();
  const client = useQueryClient();
  const wallets = useWallets();
  const list = wallets.data ?? [];
  const gap = useHistoryGap();
  const model = useModel(historyGapModel(b));
  const titleId = useId();
  const { state } = model;
  if (!gap.data || gap.data.walletIds.length === 0 || model.hides(gap.data.walletIds)) return null;
  const working = state.kind === "working";
  return (
    <section className="protocol-advisory-banner history-gap-notice card-l1" data-testid="history-gap-notice" aria-labelledby={titleId}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <h2 className="history-gap-title" id={titleId}>{HistoryGapText.title}</h2>
        <p className="protocol-advisory-text" data-testid="history-gap-text">
          {HistoryGapText.body(gap.data, list)}
        </p>
        <p className="protocol-advisory-text" data-testid="history-gap-action">
          {HistoryGapText.action}
        </p>
        {state.kind === "failed" && (
          <p className="field-error" role="alert" data-testid="history-gap-error">
            {state.text}
          </p>
        )}
        <div className="history-gap-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="history-gap-reset" disabled={working} onClick={() => model.ask()}>
            {HistoryGapText.reset}
          </DashButton>
          <DashButton variant="plainBlue" size="small" data-testid="history-gap-not-now" disabled={working} onClick={() => model.notNow(gap.data!.walletIds)}>
            {HistoryGapText.notNow}
          </DashButton>
        </div>
      </div>
      {state.kind === "confirming" && (
        <Sheet title={RepairText.resetChainData} width="small" testId="history-gap-confirm" onClose={() => model.dismiss()}>
          <p className="sheet-text">{RepairText.resetChainDataQuestion}</p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" size="large" data-testid="history-gap-confirm-cancel" onClick={() => model.dismiss()}>
              {tr("common.cancel")}
            </DashButton>
            <DashButton
              variant="filledBlue"
              size="large"
              data-testid="history-gap-confirm-ok"
              onClick={() =>
                void model.confirm().then((done) => {
                  // The shell reads it again once sync has started; the old gap is gone now.
                  if (done) client.setQueryData(keys.historyGap, null);
                })
              }
            >
              {RepairText.resetAction}
            </DashButton>
          </div>
        </Sheet>
      )}
    </section>
  );
}
