// "Move to Dash wallet" (platform/move.ts): the steps Transfer shows for it (the quote, Confirm & move,
// progress and outcome), and what holds a move back.

import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { creditsToDuffs } from "../api/pending";
import { usePlatformActivity, usePlatformMoveQuote } from "../api/queries";
import { navigate, useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import { useDisplay } from "../format/display";
import { feeWarning, feeWarningText } from "../format/feeWarning";
import { useModel } from "../onboarding/model";
import { hasUnansweredOp, moveBlock, sendHolds, moveCanRetry, moveErrorText, plannedWithdrawalCredits, MOVE_COPY as C, type MoveBlock, type PlatformMoveFlow } from "../platform/move";

function ConfirmStep({ flow, walletId, credits, amount, block }: { flow: PlatformMoveFlow; walletId: string; credits: (c: string) => string; amount: (duffs: number) => string; block: MoveBlock | null }) {
  const quote = usePlatformMoveQuote(walletId, true);
  const activity = usePlatformActivity(walletId);
  const q = quote.data;
  const warning = q ? feeWarning(BigInt(q.feeCredits), BigInt(plannedWithdrawalCredits(q))) : null;
  return (
    <>
      {block === "pending" ? (
        // A move turned up while this confirmation was open (a late Activity read): it is not offered again.
        <p className="form-problem" role="alert" data-testid="platform-move-blocked">
          {C.waiting.text}
        </p>
      ) : (
        <p className="sheet-text">{C.message}</p>
      )}
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="platform-move-quote-error">
          {moveErrorText(quote.error, credits, activity.data)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {C.quoting}
        </p>
      ) : (
        <dl className="detail-list topup-quote" data-testid="platform-move-quote">
          <div className="detail-row">
            <dt className="detail-label">{C.rows.taken}</dt>
            <dd className="detail-value" data-testid="platform-move-quote-credits">
              {credits(plannedWithdrawalCredits(q))}
            </dd>
          </div>
          <div className="detail-row">
            <dt className="detail-label">{C.rows.fee}</dt>
            <dd className="detail-value" data-testid="platform-move-quote-fee">
              {credits(q.feeCredits)}
            </dd>
          </div>
          <div className="detail-row topup-total">
            <dt className="detail-label">{C.rows.receive}</dt>
            <dd className="detail-value" data-testid="platform-move-quote-duffs">
              {amount(q.expectedDuffs)}
            </dd>
          </div>
          <div className="detail-row">
            <dt className="detail-label">{C.rows.to}</dt>
            <dd className="detail-value withdraw-to" data-testid="platform-move-quote-to">
              {q.to}
            </dd>
          </div>
        </dl>
      )}
      {warning && (
        <p className="fee-warning" role="alert" data-testid="platform-move-fee-warning">
          {feeWarningText(warning)}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="platform-move-cancel" disabled={flow.running} onClick={() => flow.close()}>
          {C.cancel}
        </DashButton>
        {block === "pending" ? (
          <DashButton variant="filledBlue" size="large" data-testid="platform-move-blocked-view" onClick={() => { flow.close(); navigate("activity"); }}>
            {C.waiting.action}
          </DashButton>
        ) : (
          <DashButton
            variant="filledBlue"
            size="large"
            data-testid="platform-move-go"
            why={block === "checking" ? C.check.loading : block === "failed" ? C.check.failed : quote.isError ? moveErrorText(quote.error, credits, activity.data) : flow.running ? C.running.title : C.quoting}
            disabled={block !== null || !q || quote.isError || quote.isFetching || flow.running}
            onClick={() => block === null && q && void flow.move(q.grant, q.to)}
          >
            {C.go}
          </DashButton>
        )}
      </div>
    </>
  );
}

/** The steps of a move: the quote's confirmation, progress and outcome (Transfer shows them,
 * TransferSheet.tsx). */
export function MoveBody({ flow, walletId, network, block }: { flow: PlatformMoveFlow; walletId: string; network: NetworkInfo; block: MoveBlock | null }) {
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const activity = usePlatformActivity(walletId);
  const amount = (duffs: number) => (discreet ? shown.masked() : `${shown.compact(duffs)} ${shown.name}`);
  const credits = (c: string) => amount(creditsToDuffs(c));
  const { step, outcome } = flow;
  return (
    <div data-testid="platform-move-body" data-step={step}>
      {step === "confirm" && <ConfirmStep flow={flow} walletId={walletId} credits={credits} amount={amount} block={block} />}
      {step === "running" && (
        <div role="status" className="topup-running">
          <p className="topup-title">
            {C.running.title}
            <span className="spinner" aria-hidden="true" />
          </p>
          <p className="sheet-text">{C.running.text}</p>
        </div>
      )}
      {step === "done" && outcome && (
        <div role="status">
          <p className="topup-title" data-testid="platform-move-outcome-title">
            {outcome.state === "done" ? C.done.title : outcome.state === "replaced" ? C.replaced.title : C.pending.title}
          </p>
          <p className="sheet-text" data-testid="platform-move-outcome-text">
            {outcome.state === "replaced"
              ? C.replaced.text
              : outcome.state === "done"
              ? outcome.expectedDuffs === null
                ? C.done.debitOnly(credits(outcome.credits))
                : C.done.text(amount(outcome.expectedDuffs), outcome.feeCredits === null ? null : credits(outcome.feeCredits))
              : outcome.expectedDuffs === null
                ? C.pending.debitOnly(credits(outcome.credits))
                : C.pending.text(amount(outcome.expectedDuffs))}
          </p>
          <div className="sheet-actions">
            <DashButton variant="filledBlue" size="large" data-testid="platform-move-done" onClick={() => flow.close()}>
              {C.close}
            </DashButton>
          </div>
        </div>
      )}
      {step === "refused" && (
        <>
          <p className="form-problem" role="alert" data-testid="platform-move-refusal">
            {moveErrorText(flow.error, credits, activity.data)}
          </p>
          <div className="sheet-actions">
            {moveCanRetry(flow.error) && (
              <DashButton variant="tintedGray" size="large" data-testid="platform-move-refused-back" onClick={() => flow.back()}>
                {C.back}
              </DashButton>
            )}
            <DashButton variant="filledBlue" size="large" data-testid="platform-move-close" onClick={() => flow.close()}>
              {C.close}
            </DashButton>
          </div>
        </>
      )}
    </div>
  );
}

/** What holds a move back on `flow`'s wallet, from Activity: a move Platform has not confirmed (this
 * session's result, or after a restart Activity's record), a payment that may still go through, and
 * whether Activity has answered at all. */
export function usePlatformHold(walletId: string, flow: PlatformMoveFlow) {
  useModel(flow);
  const activity = usePlatformActivity(walletId);
  // Only an unanswered move or send waits here: one that failed by height and may still land leaves Move
  // offered (a move at the same inputs follows a held move and frees it); the engine refuses the rest.
  const waiting = flow.pending || hasUnansweredOp(activity.data);
  const sendHeld = !flow.pending && sendHolds(activity.data);
  return { activity, waiting, sendHeld, block: moveBlock(waiting, activity), waitingText: sendHeld ? C.waiting.sendHeld : C.waiting.text };
}
