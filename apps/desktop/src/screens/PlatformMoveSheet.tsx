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
import { feeWarning, feeWarningText } from "../format/feeWarning";
import { useModel } from "../onboarding/model";
import { hasUnansweredOp, moveBlock, sendHolds, moveCanRetry, moveErrorText, moveFor, MOVE_COPY as C, type MoveBlock, type PlatformMoveFlow } from "../platform/move";
import { GrantSheet } from "./tools/common";

function ConfirmStep({ flow, walletId, credits, amount, block }: { flow: PlatformMoveFlow; walletId: string; credits: (c: string) => string; amount: (duffs: number) => string; block: MoveBlock | null }) {
  const quote = usePlatformMoveQuote(walletId, true);
  const q = quote.data;
  const warning = q ? feeWarning(BigInt(q.feeCredits), BigInt(q.credits)) : null;
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
            why={block === "checking" ? C.check.loading : block === "failed" ? C.check.failed : quote.isError ? moveErrorText(quote.error, credits) : flow.running ? C.running.title : C.quoting}
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

export function PlatformMoveSheet({ flow, walletId, network, block }: { flow: PlatformMoveFlow; walletId: string; network: NetworkInfo; block: MoveBlock | null }) {
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
export function PlatformMoveCard({ walletId, network, platform, dustCredits = "0" }: { walletId: string; network: NetworkInfo; platform: number | null | undefined; dustCredits?: string }) {
  const backend = useBackend();
  const flow = moveFor(backend, walletId);
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const activity = usePlatformActivity(walletId);
  // Activity settles a retained `pending` result: when it changes, when a move turns pending (the
  // rows read may already have shown it settled) and when the sheet closes.
  useEffect(() => flow.settle(activity.data), [flow, activity.data, flow.pending, flow.open]);
  const funded = platform != null && platform > 0;
  const dust = BigInt(dustCredits) > 0n;
  const dustText = C.card.dust(discreet ? shown.masked() : `${shown.compact(creditsToDuffs(dustCredits))} ${shown.name}`);
  // A move Platform has not confirmed: this session's result, or (after a restart) Activity's record.
  // Until Activity has answered, a fresh Move is not offered either (and an open confirmation waits).
  // Only an unanswered move or send waits here: one that failed by height and may still land leaves Move
  // offered (a move at the same inputs follows a held move and frees it); the engine refuses the rest.
  const waiting = flow.pending || hasUnansweredOp(activity.data);
  const sendHeld = !flow.pending && sendHolds(activity.data);
  const block = moveBlock(waiting, activity);
  const sheet = <PlatformMoveSheet flow={flow} walletId={walletId} network={network} block={block} />;
  const holdReason = flow.running ? C.running.text : block === "checking" ? C.check.loading : block === "failed" ? C.check.failed : null;
  if (!flow.open && !flow.running && !flow.unseen && !funded && !waiting && !dust) return sheet;
  return (
    <>
      <section className="platform-card card-l1" data-testid="platform-card" aria-label={C.card.title}>
        <div className="platform-card-body">
          <h2 className="platform-card-title">{C.card.title}</h2>
          <p className="platform-card-text">
            {funded && <span data-testid="platform-card-amount">{discreet ? shown.masked() : shown.withUnit(platform)}. </span>}
            {waiting ? (sendHeld ? C.waiting.sendHeld : C.waiting.text) : flow.unseen ? C.unseen : funded ? C.card.text : dustText}
          </p>
          {sendHeld && !waiting && (
            <p className="platform-card-text" data-testid="platform-card-send-held">
              {C.waiting.sendHeld}
            </p>
          )}
          {funded && dust && !waiting && (
            <p className="platform-card-text" data-testid="platform-card-dust">
              {dustText}
            </p>
          )}
          {holdReason && !waiting && (
            <p className="field-hint" id="platform-move-why" data-testid="platform-move-why">
              {holdReason}
              {block === "failed" && !flow.running && (
                <>
                  {" "}
                  <DashButton variant="plainBlue" size="small" data-testid="platform-move-check-again" onClick={() => void activity.refetch()}>
                    {C.check.again}
                  </DashButton>
                </>
              )}
            </p>
          )}
        </div>
        {waiting && !flow.running ? (
          <DashButton variant="tintedBlue" data-testid="platform-move-view" onClick={() => navigate("activity")}>
            {C.waiting.action}
          </DashButton>
        ) : funded || flow.running || flow.unseen || flow.open ? (
          <DashButton variant="tintedBlue" data-testid="platform-move" disabled={flow.running || (!flow.unseen && (block === "checking" || block === "failed"))} why={flow.unseen ? null : holdReason} whyId="platform-move-why" onClick={() => flow.begin()}>
            {flow.running ? C.running.title : flow.unseen ? C.show : C.action}
          </DashButton>
        ) : null}
      </section>
      {sheet}
    </>
  );
}
