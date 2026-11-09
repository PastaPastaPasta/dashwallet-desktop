// "Add from Dash wallet" (platform/fund.ts): the quote's confirmation, progress and outcome, which
// Transfer shows after its form (TransferSheet.tsx). Home's Platform card (PlatformMoveSheet.tsx)
// lists the locks that were not applied yet.

import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { PendingPlatformFund } from "../api/generated/PendingPlatformFund";
import { CREDITS_PER_DUFF, creditsToDuffs } from "../api/pending";
import { usePendingPlatformFunds, usePlatformFundQuote } from "../api/queries";
import { useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import { useDisplay } from "../format/display";
import { feeWarning, feeWarningText } from "../format/feeWarning";
import { useModel } from "../onboarding/model";
import { endedBeforeApply, finishAfter, fundCanChange, fundErrorText, FUND_COPY as C, type PlatformFundFlow } from "../platform/fund";

/** Formats for the sheet: Dash in the display unit, masked in discreet mode. */
function useFormats(network: NetworkInfo) {
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const duffs = (d: number) => (discreet ? shown.masked() : `${shown.compact(d)} ${shown.name}`);
  return { shown, duffs, credits: (c: string) => duffs(creditsToDuffs(c)) };
}

type Formats = ReturnType<typeof useFormats>;

function Row({ label, value, testId, total }: { label: string; value: string; testId: string; total?: boolean }) {
  return (
    <div className={`detail-row ${total ? "topup-total" : ""}`}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value" data-testid={testId}>
        {value}
      </dd>
    </div>
  );
}

function ConfirmStep({ flow, walletId, fmt }: { flow: PlatformFundFlow; walletId: string; fmt: Formats }) {
  const request = flow.request?.kind === "add" ? flow.request : null;
  const quote = usePlatformFundQuote(walletId, request?.duffs ?? null);
  const q = quote.data;
  const { duffs, credits } = fmt;
  // The fees (the lock's and Platform's) against what is added.
  const warning = q ? feeWarning(BigInt(q.feeDuffs) * CREDITS_PER_DUFF + BigInt(q.feeCredits), BigInt(q.duffs) * CREDITS_PER_DUFF) : null;
  return (
    <>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="platform-fund-quote-error">
          {fundErrorText(quote.error, credits, duffs)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {C.confirm.quoting}
        </p>
      ) : (
        <dl className="detail-list topup-quote" data-testid="platform-fund-confirm">
          <Row label={C.confirm.rows.from} value={C.confirm.rows.fromValue} testId="platform-fund-confirm-from" />
          <div className="detail-row">
            <dt className="detail-label">{C.confirm.rows.to}</dt>
            <dd className="detail-value">
              <span data-testid="platform-fund-confirm-to-label">{C.confirm.rows.toValue}</span>
              <span className="withdraw-to destination-line" data-testid="platform-fund-confirm-to">
                {q.to}
              </span>
            </dd>
          </div>
          <Row label={C.confirm.rows.amount} value={duffs(q.duffs)} testId="platform-fund-confirm-amount" />
          <Row label={C.confirm.rows.fee} value={duffs(q.feeDuffs)} testId="platform-fund-confirm-fee" />
          <Row label={C.confirm.rows.total} value={duffs(q.totalDuffs)} testId="platform-fund-confirm-total" total />
          <Row label={C.confirm.rows.platformFee} value={credits(q.feeCredits)} testId="platform-fund-confirm-platform-fee" />
          <Row label={C.confirm.rows.receive} value={credits(q.credits)} testId="platform-fund-confirm-receive" total />
        </dl>
      )}
      {warning && (
        <p className="fee-warning" role="alert" data-testid="platform-fund-fee-warning">
          {feeWarningText(warning)}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="platform-fund-back" disabled={flow.running} onClick={() => flow.back()}>
          {C.confirm.back}
        </DashButton>
        <DashButton
          variant="filledBlue"
          size="large"
          data-testid="platform-fund-go"
          why={quote.isError ? C.confirm.quoteFailed : flow.running ? C.running.title : C.confirm.quoting}
          disabled={!q || quote.isError || quote.isFetching || flow.running}
          onClick={() => q && void flow.run(q.grant)}
        >
          {C.confirm.go}
        </DashButton>
      </div>
    </>
  );
}

function Outcome({ flow, fmt, pending }: { flow: PlatformFundFlow; fmt: Formats; pending: readonly PendingPlatformFund[] | null | undefined }) {
  const o = flow.outcome;
  if (!o) return null;
  const s = o.state;
  const r = flow.request;
  const duffs = r?.kind === "add" ? r.duffs : pending?.find((p) => p.txid === o.txid)?.duffs;
  const ended = s.kind === "notApplied" ? endedBeforeApply(s.error, duffs === undefined ? null : fmt.duffs(duffs)) : null;
  const { title, text } =
    s.kind === "applied"
      ? { title: C.done.applied.title, text: C.done.applied.text(fmt.credits(s.credits)) }
      : s.kind === "proofWaiting"
        ? C.done.waiting
        : s.kind === "consumptionUnknown"
          ? C.done.checking
          : { title: C.done.notApplied.title, text: ended ?? C.done.notApplied.text(s.error) };
  return (
    <div role="status">
      <p className="topup-title" data-testid="platform-fund-outcome-title">
        {title}
      </p>
      <p className="sheet-text" data-testid="platform-fund-outcome-text">
        {text}
      </p>
      <div className="sheet-actions">
        {ended && (
          <DashButton variant="filledBlue" size="large" data-testid="platform-fund-outcome-finish" onClick={() => void flow.resume(o.txid)}>
            {C.approvalEnded.finish}
          </DashButton>
        )}
        <DashButton variant={ended ? "tintedGray" : "filledBlue"} size="large" data-testid="platform-fund-done" onClick={() => flow.close()}>
          {C.close}
        </DashButton>
      </div>
    </div>
  );
}

/** The steps of an add after its form (Transfer's, TransferSheet.tsx): the quote's confirmation,
 * progress and outcome, and Finish for a lock its approval left unapplied. */
export function FundBody({ flow, walletId, network }: { flow: PlatformFundFlow; walletId: string; network: NetworkInfo }) {
  useModel(flow);
  const fmt = useFormats(network);
  const pending = usePendingPlatformFunds(walletId).data;
  const { step } = flow;
  const ctx = { request: flow.request, pending };
  const retry = step === "refused" ? finishAfter(flow.error, ctx) : null;
  return (
    <div data-testid="platform-fund-body" data-step={step}>
      {step === "confirm" && <ConfirmStep flow={flow} walletId={walletId} fmt={fmt} />}
      {step === "running" && (
        <div role="status" className="topup-running">
          <p className="topup-title">
            {C.running.title}
            <span className="spinner" aria-hidden="true" />
          </p>
          <p className="sheet-text" data-testid="platform-fund-progress">
            {C.running.steps.join(" → ")}
          </p>
          <p className="sheet-text">{C.running.text}</p>
        </div>
      )}
      {step === "done" && <Outcome flow={flow} fmt={fmt} pending={pending} />}
      {step === "refused" && (
        <>
          <p className="form-problem" role="alert" data-testid="platform-fund-refusal">
            {fundErrorText(flow.error, fmt.credits, fmt.duffs, ctx)}
          </p>
          <div className="sheet-actions">
            {retry && (
              <DashButton variant="filledBlue" size="large" data-testid="platform-fund-refused-finish" onClick={() => void flow.resume(retry.txid)}>
                {retry.action === "finish" ? C.approvalEnded.finish : C.approvalEnded.checkAgain}
              </DashButton>
            )}
            {fundCanChange(flow.error) && flow.request?.kind !== "resume" && (
              <DashButton variant="tintedGray" size="large" data-testid="platform-fund-refused-back" onClick={() => flow.back()}>
                {C.back}
              </DashButton>
            )}
            <DashButton variant={retry ? "tintedGray" : "filledBlue"} size="large" data-testid="platform-fund-close" onClick={() => flow.close()}>
              {C.close}
            </DashButton>
          </div>
        </>
      )}
    </div>
  );
}
