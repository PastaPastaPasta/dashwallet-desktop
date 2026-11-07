// "Move mixed coins" (IOS-057, 0.2 MIX-SHIELD): to the wallet, the plan's fully and not fully mixed
// amounts, the fee and the number of transactions; into Shielded, the lock's fees and what Shielded
// gains, or the consolidation round more than 50 coins need first. Stop Mixing when mixing runs,
// progress, and the outcome. The flow is coinjoin/move.ts; the sheet can close while the move runs
// and reopen to its result.

import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { creditsToDuffs } from "../../api/pending";
import {
  canRetry,
  combineResultText,
  consolidationFee,
  MOVE_COPY,
  moveErrorText,
  planFee,
  planReceives,
  resultText,
  shieldResultText,
  type MoveMixedFlow,
} from "../../coinjoin/move";
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
  const credits = (c: string) => amount(creditsToDuffs(c));
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step, plan, result, shieldQuote: quote, consolidation } = flow;
  const outcome =
    step !== "done"
      ? null
      : flow.shielded
        ? { kind: "shielded" as const, ...shieldResultText(flow.shielded, credits) }
        : result && flow.combined
          ? { ...combineResultText(result, amount), combined: true }
          : result
            ? resultText(result, amount)
            : null;
  const replanned = flow.replanned && (
    <p className="form-problem" role="alert" data-testid="move-mixed-replanned">
      {MOVE_COPY.planChanged}
    </p>
  );
  const mixedRows = (fully: number, partly: number) => (
    <>
      <Row label={MOVE_COPY.confirm.fullyMixed} value={amount(fully)} testId="move-mixed-fully" />
      <Row label={MOVE_COPY.confirm.notFullyMixed} value={amount(partly)} testId="move-mixed-partly" />
    </>
  );
  const combinedNote = (fully: number) =>
    fully > 0 && (
      <p className="sheet-text" data-testid="move-mixed-combined">
        {MOVE_COPY.confirm.combined}
      </p>
    );
  const waiting = (n: number) =>
    n > 0 && (
      <p className="sheet-text" data-testid="move-mixed-waiting">
        {MOVE_COPY.confirm.waiting(n)}
      </p>
    );
  const actions = (go: string, testId: string, onGo: () => void) => (
    <div className="sheet-actions">
      <DashButton variant="tintedGray" size="large" data-testid="move-mixed-cancel" disabled={flow.running} onClick={() => flow.close()}>
        {MOVE_COPY.confirm.cancel}
      </DashButton>
      <DashButton variant="filledBlue" size="large" data-testid={testId} disabled={flow.running} onClick={onGo}>
        {go}
      </DashButton>
    </div>
  );
  const close = (
    <DashButton variant="filledBlue" size="large" data-testid="move-mixed-close" onClick={() => flow.close()}>
      {MOVE_COPY.close}
    </DashButton>
  );
  return (
    <>
      <Sheet title={MOVE_COPY.title} width="small" testId="move-mixed-sheet" onClose={() => flow.close()}>
        <div data-testid="move-mixed-body" data-step={step} data-destination={flow.destination} data-outcome={outcome?.kind}>
          {step === "plan" && (
            <p className="sheet-text" role="status">
              {MOVE_COPY.planning}
              <span className="spinner" aria-hidden="true" />
            </p>
          )}
          {step === "confirm" && plan && (
            <>
              {replanned}
              <p className="sheet-text">{MOVE_COPY.confirm.message}</p>
              <dl className="detail-list topup-quote" data-testid="move-mixed-plan">
                {mixedRows(plan.fullyMixed, plan.notFullyMixed)}
                <Row label={MOVE_COPY.confirm.fee} value={amount(planFee(plan))} testId="move-mixed-fee" />
                <Row label={MOVE_COPY.confirm.receive} value={amount(planReceives(plan))} testId="move-mixed-receive" total />
              </dl>
              {combinedNote(plan.fullyMixed)}
              <p className="sheet-text" data-testid="move-mixed-transactions">
                {MOVE_COPY.confirm.transactions(plan.chunks.length)}
              </p>
              {waiting(plan.unconfirmed)}
              {actions(MOVE_COPY.confirm.move, "move-mixed-confirm", () => void flow.move())}
            </>
          )}
          {step === "confirm" && quote && (
            <>
              {replanned}
              <p className="sheet-text">{MOVE_COPY.shielded.message}</p>
              <dl className="detail-list topup-quote" data-testid="move-mixed-plan">
                {mixedRows(quote.fullyMixedDuffs, quote.notFullyMixedDuffs)}
                <Row label={MOVE_COPY.shielded.fee} value={amount(quote.feeDuffs)} testId="move-mixed-fee" />
                <Row label={MOVE_COPY.shielded.poolFee} value={credits(quote.poolFeeCredits)} testId="move-mixed-pool-fee" />
                <Row label={MOVE_COPY.shielded.gain} value={credits(quote.credits)} testId="move-mixed-receive" total />
              </dl>
              {combinedNote(quote.fullyMixedDuffs)}
              {actions(MOVE_COPY.confirm.move, "move-mixed-confirm", () => void flow.move())}
            </>
          )}
          {step === "combine" && consolidation && (
            <>
              {replanned}
              <p className="sheet-text" data-testid="move-mixed-combine-text">
                {MOVE_COPY.combine.text(consolidation.chunks.reduce((n, c) => n + c.inputs, 0))}
              </p>
              <dl className="detail-list topup-quote" data-testid="move-mixed-plan">
                {mixedRows(consolidation.fullyMixed, consolidation.notFullyMixed)}
                <Row label={MOVE_COPY.confirm.fee} value={amount(consolidationFee(consolidation))} testId="move-mixed-fee" />
                <Row label={MOVE_COPY.combine.after} value={String(consolidation.coinsAfter)} testId="move-mixed-coins-after" total />
              </dl>
              <p className="sheet-text" data-testid="move-mixed-transactions">
                {MOVE_COPY.confirm.transactions(consolidation.chunks.length)}
              </p>
              {waiting(consolidation.unconfirmed)}
              {actions(MOVE_COPY.combine.go, "move-mixed-combine", () => void flow.combine())}
            </>
          )}
          {step === "mixing" && (
            <>
              <p className="sheet-text" role="alert" data-testid="move-mixed-mixing">
                {MOVE_COPY.mixing.text}
              </p>
              {actions(MOVE_COPY.mixing.stop, "move-mixed-stop", () => void flow.stopMixing())}
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
              {outcome.kind === "unknown" && flow.resent && (
                <p className="sheet-text" data-testid="move-mixed-resent" data-state={flow.resent}>
                  {MOVE_COPY.resent[flow.resent]}
                </p>
              )}
              <div className="sheet-actions">
                {outcome.kind === "unknown" && flow.resent !== "resolved" && (
                  <DashButton variant="tintedGray" size="large" data-testid="move-mixed-resend" disabled={flow.running} onClick={() => void flow.resend()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {outcome.kind === "partial" && (
                  <DashButton variant="tintedGray" size="large" data-testid="move-mixed-again" onClick={() => flow.again()}>
                    {MOVE_COPY.again}
                  </DashButton>
                )}
                {"combined" in outcome && outcome.kind === "done" && (
                  <DashButton variant="tintedGray" size="large" data-testid="move-mixed-shield-next" onClick={() => flow.again()}>
                    {MOVE_COPY.card.shield}
                  </DashButton>
                )}
                {close}
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="move-mixed-refusal">
                {moveErrorText(flow.error, amount)}
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
