// "Return to Core": an explicit, fee-confirmed self-payment that returns CoinJoin-account coins
// held after a mixing session (legacy or hand-off) to fresh Core receive addresses of this wallet.

import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import {
  canRetry,
  MOVE_COPY,
  planFee,
  planReceives,
  RETURN_COPY,
  returnRefusalText,
  returnResultText,
  type ReturnToCoreFlow,
} from "../../coinjoin/move";
import { useCoinJoinAmount, useMaskedAmount } from "../../coinjoin/queries";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

function Row({ label, value, testId, total }: { label: string; value: string; testId: string; total?: boolean }) {
  return (
    <div className={total ? "detail-row topup-total" : "detail-row"}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value" data-testid={testId}>
        <bdi dir="ltr">{value}</bdi>
      </dd>
    </div>
  );
}

export function ReturnToCoreSheet({ flow, network }: { flow: ReturnToCoreFlow; network: NetworkInfo }) {
  useT();
  useModel(flow);
  const plain = useCoinJoinAmount(network);
  const masked = useMaskedAmount(network);
  const amount = (duffs: number) => masked ?? plain(duffs);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step, plan, result } = flow;
  const outcome = step === "done" && result ? returnResultText(result, amount) : null;
  const close = (
    <DashButton variant="filledBlue" size="large" data-testid="return-to-core-close" onClick={() => flow.close()}>
      {MOVE_COPY.close}
    </DashButton>
  );
  return (
    <>
      <Sheet title={RETURN_COPY.title} width="small" testId="return-to-core-sheet" onClose={() => flow.close()}>
        <div data-testid="return-to-core-body" data-step={step} data-outcome={outcome?.kind}>
          {step === "plan" && (
            <p className="sheet-text" role="status">
              {RETURN_COPY.planning}
              <span className="spinner" aria-hidden="true" />
            </p>
          )}
          {step === "confirm" && plan && (
            <>
              {flow.replanned && (
                <p className="form-problem" role="alert" data-testid="return-to-core-replanned">
                  {RETURN_COPY.planChanged}
                </p>
              )}
              <p className="sheet-text">{RETURN_COPY.confirm.message}</p>
              <dl className="detail-list topup-quote" data-testid="return-to-core-plan">
                <Row label={RETURN_COPY.confirm.heldAmount} value={amount(plan.total)} testId="return-to-core-amount" />
                <Row label={RETURN_COPY.confirm.destination} value={RETURN_COPY.confirm.destinationCore} testId="return-to-core-destination" />
                <Row label={MOVE_COPY.confirm.fee} value={amount(planFee(plan))} testId="return-to-core-fee" />
                <Row label={MOVE_COPY.confirm.receive} value={amount(planReceives(plan))} testId="return-to-core-receive" total />
              </dl>
              {plan.fullyMixed > 0 && (
                <p className="sheet-text" data-testid="return-to-core-combined">
                  {MOVE_COPY.confirm.combined}
                </p>
              )}
              <p className="sheet-text" data-testid="return-to-core-transactions">
                {MOVE_COPY.confirm.transactions(plan.chunks.length)}
              </p>
              {plan.unconfirmed > 0 && (
                <p className="sheet-text" data-testid="return-to-core-waiting">
                  {MOVE_COPY.confirm.waiting(plan.unconfirmed)}
                </p>
              )}
              <div className="sheet-actions">
                <DashButton variant="tintedGray" size="large" data-testid="return-to-core-cancel" disabled={flow.running} onClick={() => flow.close()}>
                  {MOVE_COPY.confirm.cancel}
                </DashButton>
                <DashButton variant="filledBlue" size="large" data-testid="return-to-core-confirm" disabled={flow.running} onClick={() => void flow.returnNow()}>
                  {RETURN_COPY.confirm.action}
                </DashButton>
              </div>
            </>
          )}
          {step === "mixing" && (
            <>
              <p className="sheet-text" role="alert" data-testid="return-to-core-mixing">
                {RETURN_COPY.mixing.text}
              </p>
              <div className="sheet-actions">
                <DashButton variant="tintedGray" size="large" data-testid="return-to-core-cancel" disabled={flow.running} onClick={() => flow.close()}>
                  {MOVE_COPY.confirm.cancel}
                </DashButton>
                <DashButton variant="filledBlue" size="large" data-testid="return-to-core-stop" disabled={flow.running} onClick={() => void flow.stopMixing()}>
                  {MOVE_COPY.mixing.stop}
                </DashButton>
              </div>
            </>
          )}
          {step === "running" && (
            <div role="status" className="topup-running">
              <p className="topup-title">
                {RETURN_COPY.running.title}
                <span className="spinner" aria-hidden="true" />
              </p>
              <p className="sheet-text">{MOVE_COPY.running.text}</p>
            </div>
          )}
          {outcome && (
            <div role="status">
              <p className="topup-title" data-testid="return-to-core-outcome-title">
                {outcome.title}
              </p>
              <p className="sheet-text" data-testid="return-to-core-outcome-text">
                {outcome.text}
              </p>
              {outcome.kind === "unknown" && flow.resent && (
                <p className="sheet-text" data-testid="return-to-core-resent" data-state={flow.resent}>
                  {MOVE_COPY.resent[flow.resent]}
                </p>
              )}
              <div className="sheet-actions">
                {outcome.kind === "unknown" && flow.resent !== "resolved" && (
                  <DashButton variant="tintedGray" size="large" data-testid="return-to-core-resend" disabled={flow.running} onClick={() => void flow.resend()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {outcome.kind === "partial" && (
                  <DashButton variant="tintedGray" size="large" data-testid="return-to-core-again" onClick={() => flow.again()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {close}
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="return-to-core-refusal">
                {returnRefusalText(flow)}
              </p>
              <div className="sheet-actions">
                {canRetry(flow.error) && (
                  <DashButton variant="tintedGray" size="large" data-testid="return-to-core-again" onClick={() => flow.again()}>
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
