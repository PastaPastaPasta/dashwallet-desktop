// The details of a Shielded or Platform-address row in Activity. Such a row has no Core transaction,
// so the sheet shows its own activity record, from the reads Activity already holds.

import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import { errorMessage, hasCode } from "../api/backend";
import { creditsToDuffs } from "../api/pending";
import { useBackend, usePlatformActivity, usePlatformAddresses, useShieldedHistory, useShieldedStatus } from "../api/queries";
import { ui } from "../app/ui";
import { AmountText, DashButton, Icon } from "../components/basics";
import { CopyValue, DetailRow } from "../components/detail";
import { LoadingState } from "../components/history";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { detailDate, isProblem, rowDate, shownAmount } from "../format/tx";
import { useModel } from "../onboarding/model";
import { cancelFor, cancelResult, CANCEL_COPY, recipientOf, type CancelKind, type PlatformCancelFlow } from "../platform/cancel";
import { addressSendsOff } from "../platform/mainnetOff";
import { cancelOf, isHeldCancel, isHeldSend, MOVE_COPY, moveErrorText } from "../platform/move";
import { moveToDashWallet } from "./dashpay/DashPayCard";
import { GrantSheet } from "./tools/common";
import { findSourceRecord, heldActivities, isMayLandRow, isReplacedRow, isSettledUnknownRow, rowSource, settledUnknownText, SourceRowText, sourceRowTitle, type SourceRecord } from "../history/sourceRows";
import { useT } from "../i18n";

function Address({ value }: { value: string }) {
  const t = useT();
  return <CopyValue value={value} action={t("activity.sourceDetail.copyAddress")} mono />;
}

/** "Move to Dash wallet": the way out of a held send or cancel, or of a held move or contact payment
 * the cancel refused (too small, or its re-send refused). */
function MoveButton({ onMove }: { onMove: () => void }) {
  const t = useT();
  return (
    <DashButton variant="filledBlue" size="large" data-testid="source-detail-move-to-core" onClick={onMove}>
      {t("activity.sourceDetail.moveToCore")}
    </DashButton>
  );
}

/** A cancel refusal that leaves "Move to Dash wallet" as the way out: the held item is too small to
 * cancel, or Platform's pre-check refused the cancel. */
const movesInstead = (e: unknown) => hasCode(e, "credits.send_out_of_range") || hasCode(e, "credits.preflight_refused") || hasCode(e, "credits.rules_unsupported");

/** The cancel of a held move or contact payment, in place of the details' actions: its confirmation,
 * progress and outcome. The outcome follows Activity's row of the cancel. `onMove`: Move to Dash wallet,
 * offered where the cancel is refused. */
function CancelPanel({ flow, record, kind, activity, network, onMove }: { flow: PlatformCancelFlow; record: PlatformActivity; kind: CancelKind; activity: readonly PlatformActivity[] | null | undefined; network: NetworkInfo; onMove: () => void }) {
  useModel(flow);
  const display = useDisplay(network);
  const credits = (c: string) => display.withUnit(creditsToDuffs(c));
  const recipient = recipientOf(record);
  const { step, quote } = flow;
  const refusal = (e: unknown) => (hasCode(e, "credits.send_out_of_range") ? CANCEL_COPY.tooSmall(kind, credits(record.credits), recipient) : moveErrorText(e, credits, activity));
  const move = () => {
    flow.close();
    onMove();
  };
  if (step === "running")
    return (
      <p className="sheet-text" role="status" data-testid="platform-cancel-running">
        {CANCEL_COPY.pending}
        <span className="spinner" aria-hidden="true" />
      </p>
    );
  if (step === "done" && flow.outcome) {
    const result = cancelResult(flow.outcome, activity?.find((a) => a.id === flow.outcome?.id));
    return (
      <div role="status" data-testid="platform-cancel-outcome" data-result={result}>
        <p className="sheet-text">{result === "done" ? CANCEL_COPY.done(kind, recipient) : result === "lost" ? CANCEL_COPY.lost(kind, recipient) : CANCEL_COPY.pending}</p>
        <div className="sheet-actions">
          <DashButton variant="filledBlue" size="large" data-testid="platform-cancel-close" onClick={() => flow.close()}>
            {MOVE_COPY.close}
          </DashButton>
        </div>
      </div>
    );
  }
  if (step === "refused")
    return (
      <div data-testid="platform-cancel-refused">
        <p className="form-problem" role="alert">
          {refusal(flow.error)}
        </p>
        <div className="sheet-actions">
          <DashButton variant={movesInstead(flow.error) ? "tintedGray" : "filledBlue"} size="large" data-testid="platform-cancel-close" onClick={() => flow.close()}>
            {MOVE_COPY.close}
          </DashButton>
          {movesInstead(flow.error) && <MoveButton onMove={move} />}
        </div>
      </div>
    );
  return (
    <div data-testid="platform-cancel-confirm">
      <p className="topup-title">{CANCEL_COPY.title(kind)}</p>
      {flow.quoteError ? (
        <p className="form-problem" role="alert" data-testid="platform-cancel-quote-error">
          {refusal(flow.quoteError)}
        </p>
      ) : !quote ? (
        <p className="sheet-text" role="status">
          {MOVE_COPY.quoting}
        </p>
      ) : (
        <p className="sheet-text" data-testid="platform-cancel-body">
          {CANCEL_COPY.body(kind, credits(record.credits), credits(quote.feeCredits), recipient)}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="platform-cancel-dismiss" disabled={flow.running} onClick={() => flow.dismiss()}>
          {CANCEL_COPY.dismiss}
        </DashButton>
        {movesInstead(flow.quoteError) ? (
          <MoveButton onMove={move} />
        ) : (
          <DashButton variant="filledBlue" size="large" data-testid="platform-cancel-go" disabled={!quote || !!flow.quoteError || flow.running} onClick={() => void flow.confirm()}>
            {CANCEL_COPY.confirm(kind)}
          </DashButton>
        )}
      </div>
    </div>
  );
}

/** A held Platform item's way out: Move to Dash wallet beside a held send or cancel (Transfer plans it
 * from that item's inputs), Cancel beside a held move or contact payment. */
function HeldActions({ record, walletId, network }: { record: PlatformActivity; walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const activity = usePlatformActivity(walletId).data;
  const sendsOff = addressSendsOff(network, usePlatformAddresses(walletId).data);
  const flow = cancelFor(backend, walletId);
  useModel(flow);
  const kind = cancelOf(record, activity, sendsOff);
  const onMove = () => {
    ui.set({ detail: null });
    moveToDashWallet(backend, walletId);
  };
  if (flow.of === record.id && (kind || flow.step !== "confirm"))
    return <CancelPanel flow={flow} record={record} kind={kind ?? (record.kind === "contactPayment" ? "payment" : "move")} activity={activity} network={network} onMove={onMove} />;
  if (isHeldSend(record) || isHeldCancel(record))
    return (
      <div className="sheet-actions">
        <MoveButton onMove={onMove} />
      </div>
    );
  if (!kind) return null;
  return (
    <div className="sheet-actions">
      <DashButton variant="tintedGray" size="large" data-testid="source-detail-cancel" disabled={flow.running} onClick={() => void flow.begin(record.id)}>
        {CANCEL_COPY.action(kind)}
      </DashButton>
    </div>
  );
}

function Body({ found, network, walletId }: { found: SourceRecord; network: NetworkInfo; walletId: string }) {
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
      {found.source === "platform" && <HeldActions record={found.record} walletId={walletId} network={network} />}
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
  const cancel = cancelFor(useBackend(), walletId);
  useModel(cancel);
  return (
    <>
    <Sheet title={t("activity.sourceDetail.title")} width="medium" testId="source-detail" focusKey={`${cancel.of ?? ""}:${cancel.step}`} onClose={() => ui.set({ detail: null })}>
      {found ? (
        <Body found={found} network={network} walletId={walletId} />
      ) : read.isError ? (
        <p role="alert">{errorMessage(read.error)}</p>
      ) : read.isPending ? (
        <LoadingState testId="source-detail-loading" text={t("activity.sourceDetail.loading")} />
      ) : (
        <p data-testid="source-detail-gone">{t("activity.sourceDetail.gone")}</p>
      )}
    </Sheet>
    <GrantSheet grants={cancel.grants} />
    </>
  );
}
