// "Move to Dash wallet" (platform/move.ts): the Home card that offers it while the Platform
// addresses hold something, and the sheet with the quote, Confirm & move, progress and outcome.

import { useEffect } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { creditsToDuffs } from "../api/pending";
import { useBackend, usePlatformActivity, usePlatformMoveQuote } from "../api/queries";
import { navigate, useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { useModel } from "../onboarding/model";
import { hasPendingMove, moveCanRetry, moveErrorText, moveFor, MOVE_COPY as C, type PlatformMoveFlow } from "../platform/move";
import { GrantSheet } from "./tools/common";

function ConfirmStep({ flow, walletId, credits, amount }: { flow: PlatformMoveFlow; walletId: string; credits: (c: string) => string; amount: (duffs: number) => string }) {
  const quote = usePlatformMoveQuote(walletId, true);
  const q = quote.data;
  return (
    <>
      <p className="sheet-text">{C.message}</p>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="platform-move-quote-error">
          {moveErrorText(quote.error, credits)}
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
              {credits(q.credits)}
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
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="platform-move-cancel" disabled={flow.running} onClick={() => flow.close()}>
          {C.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="platform-move-go" why={quote.isError ? "The fee could not be worked out." : !q || quote.isFetching ? C.quoting : null} disabled={!q || quote.isError || quote.isFetching || flow.running} onClick={() => q && void flow.move(q.grant)}>
          {C.go}
        </DashButton>
      </div>
    </>
  );
}

export function PlatformMoveSheet({ flow, walletId, network }: { flow: PlatformMoveFlow; walletId: string; network: NetworkInfo }) {
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const amount = (duffs: number) => (discreet ? shown.masked() : `${shown.compact(duffs)} ${shown.name}`);
  const credits = (c: string) => amount(creditsToDuffs(c));
  const { step, outcome } = flow;
  return (
    <>
      <Sheet title={C.title} width="small" testId="platform-move-sheet" onClose={() => flow.close()}>
        <div data-testid="platform-move-body" data-step={step}>
          {step === "confirm" && <ConfirmStep flow={flow} walletId={walletId} credits={credits} amount={amount} />}
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
                {outcome.state === "done" ? C.done.title : C.pending.title}
              </p>
              <p className="sheet-text" data-testid="platform-move-outcome-text">
                {(outcome.state === "done" ? C.done : C.pending).text(amount(outcome.expectedDuffs))}
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
                {moveErrorText(flow.error, credits)}
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
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}

/** Home's Platform card: shown while the Platform addresses hold something, or a move's result
 * waits. `platform` is their balance in duffs (`null`/`undefined`: not known, no card). */
export function PlatformMoveCard({ walletId, network, platform }: { walletId: string; network: NetworkInfo; platform: number | null | undefined }) {
  const backend = useBackend();
  const flow = moveFor(backend, walletId);
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const activity = usePlatformActivity(walletId);
  // Activity settles a retained `pending` result: when it changes, when a move turns pending (the
  // rows read may already have shown it settled) and when the sheet closes.
  useEffect(() => flow.settle(activity.data), [flow, activity.data, flow.pending, flow.open]);
  const sheet = <PlatformMoveSheet flow={flow} walletId={walletId} network={network} />;
  const funded = platform != null && platform > 0;
  // A move Platform has not confirmed: this session's result, or (after a restart) Activity's record.
  const waiting = flow.pending || hasPendingMove(activity.data);
  if (!flow.open && !flow.running && !flow.unseen && !funded && !waiting) return sheet;
  return (
    <>
      <section className="platform-card card-l1" data-testid="platform-card" aria-label={C.card.title}>
        <div className="platform-card-body">
          <h2 className="platform-card-title">{C.card.title}</h2>
          <p className="platform-card-text">
            {funded && <span data-testid="platform-card-amount">{discreet ? shown.masked() : shown.withUnit(platform)}. </span>}
            {waiting ? C.waiting.text : flow.unseen ? C.unseen : C.card.text}
          </p>
        </div>
        {waiting && !flow.running ? (
          <DashButton variant="tintedBlue" data-testid="platform-move-view" onClick={() => navigate("activity")}>
            {C.waiting.action}
          </DashButton>
        ) : (
          <DashButton variant="tintedBlue" data-testid="platform-move" disabled={flow.running} why={C.running.text} onClick={() => flow.begin()}>
            {flow.running ? C.running.title : flow.unseen ? C.show : C.action}
          </DashButton>
        )}
      </section>
      {sheet}
    </>
  );
}
