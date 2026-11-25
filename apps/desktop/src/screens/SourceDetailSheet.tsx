// The details of a Shielded or Platform-address row in Activity. Such a row has no Core transaction,
// so the sheet shows its own activity record, from the reads Activity already holds.

import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { errorMessage } from "../api/backend";
import { creditsToDuffs } from "../api/pending";
import { useBackend, usePlatformActivity, useShieldedHistory, useShieldedStatus } from "../api/queries";
import { ui } from "../app/ui";
import { AmountText, Icon } from "../components/basics";
import { CopyValue, DetailRow } from "../components/detail";
import { LoadingState } from "../components/history";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { detailDate, isProblem, rowDate, shownAmount } from "../format/tx";
import { findSourceRecord, heldActivities, isMayLandRow, isReplacedRow, isSettledUnknownRow, rowSource, settledUnknownText, SourceRowText, sourceRowTitle, type SourceRecord } from "../history/sourceRows";
import { useT } from "../i18n";

function Address({ value }: { value: string }) {
  const t = useT();
  return <CopyValue value={value} action={t("activity.sourceDetail.copyAddress")} mono />;
}

function Body({ found, network }: { found: SourceRecord; network: NetworkInfo }) {
  const t = useT();
  const backend = useBackend();
  const display = useDisplay(network);
  const unit = display.name;
  const { row } = found;
  const replaced = isReplacedRow(row);
  const unknown = isSettledUnknownRow(row);
  const shown = shownAmount(row.amount, row.txType);
  const date = rowDate(row);
  const status = unknown
    ? settledUnknownText(row).text
    : replaced
      ? SourceRowText.replaced
      : isMayLandRow(row)
        ? SourceRowText.mayLand
        : t(row.status.kind === "confirmed" ? "activity.sourceDetail.status.confirmed" : row.status.kind === "notAccepted" ? "activity.sourceDetail.status.notAccepted" : "activity.sourceDetail.status.pending");
  const problem = !replaced && !unknown && !isMayLandRow(row) && isProblem(row.status);
  const fee = found.record.feeCredits;
  const refusal = found.source === "shielded" ? found.record.refusal : undefined;
  const refusalText =
    refusal === "held"
      ? shown.duffs !== 0
        ? t("activity.sourceDetail.refusal.held", { amount: display.withUnit(Math.abs(shown.duffs)) })
        : t("activity.sourceDetail.refusal.heldNoAmount")
      : refusal === "released"
        ? t("activity.sourceDetail.refusal.released")
        : null;
  return (
    <>
      <div className="tx-detail-summary" data-testid="source-detail-summary">
        <Icon id={problem ? "txDetailError" : row.amount < 0 || row.category === "sent" ? "txDetailSent" : "txDetailReceived"} size={50} />
        <h3 className="tx-detail-title" data-testid="source-detail-title">
          {sourceRowTitle(row)}
        </h3>
        {!replaced && !(unknown && row.amount === 0) && <AmountText className="tx-detail-amount" testId="source-detail-amount" text={display.full(shown.duffs, shown.signed)} unitName={unit} glyphFactor={0.8} />}
        <div className="tx-detail-status">
          {date && (
            <span className="tx-detail-date" data-testid="source-detail-when">
              {detailDate(date, backend.locale)}
            </span>
          )}
          <span className={problem ? "tx-detail-state is-problem" : "tx-detail-state"} data-testid="source-detail-status">
            {status}
          </span>
        </div>
      </div>
      {refusalText && (
        <p className="tx-detail-note" data-testid="source-detail-refusal">
          {refusalText}
        </p>
      )}
      <dl className="detail-list card-l1" data-testid="source-detail-facts">
        {found.source === "shielded" ? (
          <>
            {found.record.memo && (
              <DetailRow label={t("activity.sourceDetail.memo")} testId="source-detail-memo">
                <bdi>{found.record.memo}</bdi>
              </DetailRow>
            )}
            {found.record.counterparty && (
              <DetailRow label={t(found.record.direction === "in" ? "activity.sourceDetail.from" : "activity.sourceDetail.to")} testId="source-detail-counterparty">
                <Address value={found.record.counterparty} />
              </DetailRow>
            )}
          </>
        ) : (
          <>
            {found.record.address && (
              <DetailRow label={t("activity.sourceDetail.platformAddress")} testId="source-detail-address">
                <Address value={found.record.address} />
              </DetailRow>
            )}
            {found.record.to && (
              <DetailRow label={t("activity.sourceDetail.to")} testId="source-detail-to">
                <Address value={found.record.to} />
              </DetailRow>
            )}
          </>
        )}
        {fee !== null && (
          <DetailRow label={t(found.source === "platform" && found.record.estimated ? "activity.sourceDetail.feeEstimated" : "activity.sourceDetail.fee")} testId="source-detail-fee">
            <AmountText text={display.full(creditsToDuffs(fee))} unitName={unit} glyph={false} />
          </DetailRow>
        )}
        {row.blockHeight !== null && (
          <DetailRow label={t("activity.sourceDetail.platformBlock")} testId="source-detail-block">
            {row.blockHeight.toLocaleString(backend.locale)}
          </DetailRow>
        )}
      </dl>
    </>
  );
}

export function SourceDetailSheet({ walletId, txid, network }: { walletId: string; txid: string; network: NetworkInfo }) {
  const t = useT();
  const platform = usePlatformActivity(walletId);
  const shielded = useShieldedHistory(walletId);
  const read = rowSource({ txid }) === "platform" ? platform : shielded;
  const status = useShieldedStatus(walletId);
  const found = findSourceRecord(txid, platform.data, shielded.data, heldActivities(status.data));
  return (
    <Sheet title={t("activity.sourceDetail.title")} width="medium" testId="source-detail" onClose={() => ui.set({ detail: null })}>
      {found ? (
        <Body found={found} network={network} />
      ) : read.isError ? (
        <p role="alert">{errorMessage(read.error)}</p>
      ) : read.isPending ? (
        <LoadingState testId="source-detail-loading" text={t("activity.sourceDetail.loading")} />
      ) : (
        <p data-testid="source-detail-gone">{t("activity.sourceDetail.gone")}</p>
      )}
    </Sheet>
  );
}
