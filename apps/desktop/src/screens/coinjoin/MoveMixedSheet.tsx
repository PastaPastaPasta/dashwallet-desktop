// "Move mixed coins" to the wallet (IOS-057): the plan's fully and not fully mixed amounts, the fee
// and the number of transactions, Stop Mixing when mixing runs, progress, and the outcome. The flow
// is coinjoin/move.ts; the sheet can close while the move runs and reopen to its result.

import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { canRetry, MOVE_COPY, moveErrorText, planFee, planReceives, resultText, type MoveMixedFlow } from "../../coinjoin/move";
import { useCoinJoinAmount, useMaskedAmount } from "../../coinjoin/queries";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

function Row({ label, value, testId, total }: { label: string; value: string; testId: string; total?: boolean }) {
  return (
    <div className={total ? "detail-row topup-total" : "detail-row"}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value" data-testid={testId}>
        {value}
      </dd>
    </div>
  );
}

export function MoveMixedSheet({ flow, network }: { flow: MoveMixedFlow; network: NetworkInfo }) {
  useModel(flow);
  const plain = useCoinJoinAmount(network);
  const masked = useMaskedAmount(network);
  const amount = (duffs: number) => masked ?? plain(duffs);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step, plan, result } = flow;
  const outcome = step === "done" && result ? resultText(result, amount) : null;
  const close = (
    <DashButton variant="filledBlue" size="large" data-testid="move-mixed-close" onClick={() => flow.close()}>
      {MOVE_COPY.close}
    </DashButton>
  );
  return (
    <>
      <Sheet title={MOVE_COPY.title} width="small" testId="move-mixed-sheet" onClose={() => flow.close()}>
        <div data-testid="move-mixed-body" data-step={step} data-outcome={outcome?.kind}>
          {step === "plan" && (
            <p className="sheet-text" role="status">
              {MOVE_COPY.planning}
              <span className="spinner" aria-hidden="true" />
            </p>
          )}
          {step === "confirm" && plan && (
            <>
              <p className="sheet-text">{MOVE_COPY.confirm.message}</p>
              <dl className="detail-list topup-quote" data-testid="move-mixed-plan">
                <Row label={MOVE_COPY.confirm.fullyMixed} value={amount(plan.fullyMixed)} testId="move-mixed-fully" />
                <Row label={MOVE_COPY.confirm.notFullyMixed} value={amount(plan.notFullyMixed)} testId="move-mixed-partly" />
                <Row label={MOVE_COPY.confirm.fee} value={amount(planFee(plan))} testId="move-mixed-fee" />
                <Row label={MOVE_COPY.confirm.receive} value={amount(planReceives(plan))} testId="move-mixed-receive" total />
              </dl>
              {plan.fullyMixed > 0 && (
                <p className="sheet-text" data-testid="move-mixed-combined">
                  {MOVE_COPY.confirm.combined}
                </p>
              )}
              <p className="sheet-text" data-testid="move-mixed-transactions">
                {MOVE_COPY.confirm.transactions(plan.chunks.length)}
              </p>
              <div className="sheet-actions">
                <DashButton variant="tintedGray" size="large" data-testid="move-mixed-cancel" disabled={flow.running} onClick={() => flow.close()}>
                  {MOVE_COPY.confirm.cancel}
                </DashButton>
                <DashButton variant="filledBlue" size="large" data-testid="move-mixed-confirm" disabled={flow.running} onClick={() => void flow.move()}>
                  {MOVE_COPY.confirm.move}
                </DashButton>
              </div>
            </>
          )}
          {step === "mixing" && (
            <>
              <p className="sheet-text" role="alert" data-testid="move-mixed-mixing">
                {MOVE_COPY.mixing.text}
              </p>
              <div className="sheet-actions">
                <DashButton variant="tintedGray" size="large" data-testid="move-mixed-cancel" disabled={flow.running} onClick={() => flow.close()}>
                  {MOVE_COPY.confirm.cancel}
                </DashButton>
                <DashButton variant="filledBlue" size="large" data-testid="move-mixed-stop" disabled={flow.running} onClick={() => void flow.stopMixing()}>
                  {MOVE_COPY.mixing.stop}
                </DashButton>
              </div>
            </>
          )}
          {step === "running" && (
            <div role="status" className="topup-running">
              <p className="topup-title">
                {MOVE_COPY.running.title}
                <span className="spinner" aria-hidden="true" />
              </p>
              <p className="sheet-text">{MOVE_COPY.running.text}</p>
            </div>
          )}
          {outcome && (
            <div role="status">
              <p className="topup-title" data-testid="move-mixed-outcome-title">
                {outcome.title}
              </p>
              <p className="sheet-text" data-testid="move-mixed-outcome-text">
                {outcome.text}
              </p>
              <div className="sheet-actions">
                {outcome.kind === "partial" && (
                  <DashButton variant="tintedGray" size="large" data-testid="move-mixed-again" onClick={() => flow.again()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {close}
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="move-mixed-refusal">
                {moveErrorText(flow.error)}
              </p>
              <div className="sheet-actions">
                {canRetry(flow.error) && (
                  <DashButton variant="tintedGray" size="large" data-testid="move-mixed-again" onClick={() => flow.again()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {close}
              </div>
            </>
          )}
        </div>
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}
