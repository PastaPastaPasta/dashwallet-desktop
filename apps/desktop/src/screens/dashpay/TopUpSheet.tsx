// The top-up sheet (DASHPAY F8, IOS-131, AND-078; DP1-10 T1–T8): the amount (iOS presets, a
// custom amount), the quote's confirm step with Android's copy, progress, the outcome, and a
// refusal routed to Tools ▸ Repair when the funding's outcome is unknown. The flow is
// dashpay/topup.ts; the sheet can close while the call runs and reopen to its result (D14).

import { useId } from "react";
import { useQuery } from "@tanstack/react-query";
import type { CostTable } from "../../api/generated/CostTable";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { creditBalance, estimateText } from "../../dashpay/profile";
import { useTopUpQuote } from "../../dashpay/profileQueries";
import { amountCanChange, minTopUp, outcomeText, PRESETS, routesToRepair, TOPUP_COPY, topUpCredits, topUpErrorText, type TopUpFlow } from "../../dashpay/topup";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

/** T1: the presets and "Other amount", each with what it buys. */
function AmountStep({ flow, costs, network, amount }: { flow: TopUpFlow; costs: CostTable | undefined; network: NetworkInfo; amount: (duffs: number) => string }) {
  const b = useBackend();
  const id = useId();
  const shown = useDisplay(network);
  const { choice } = flow;
  const text = choice.kind === "other" ? choice.text.trim() : "";
  const checked = useDebounced(text, 300);
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, checked], queryFn: () => b.parseAmount(checked, shown.unit), enabled: checked !== "", staleTime: Infinity });
  const min = minTopUp(costs);
  const otherDuffs = choice.kind === "other" && checked === text && typeof parsed.data === "number" ? parsed.data : null;
  const otherProblem =
    choice.kind !== "other" || text === "" || checked !== text || parsed.isPending
      ? null
      : parsed.data === null || parsed.data === undefined
        ? TOPUP_COPY.amount.invalid
        : parsed.data < min
          ? TOPUP_COPY.amount.below(amount(min))
          : null;
  const duffs = choice.kind === "preset" ? choice.duffs : otherProblem === null ? otherDuffs : null;
  const buys = (d: number) => (costs ? estimateText(topUpCredits(d, costs), costs) : null);
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (duffs !== null) flow.next(duffs);
      }}
    >
      <p className="sheet-text">{TOPUP_COPY.amount.intro}</p>
      <fieldset className="topup-choices" aria-label={TOPUP_COPY.amount.field}>
        {PRESETS.map((p) => (
          <label key={p} className="topup-choice" data-testid={`topup-preset-${p}`}>
            <input type="radio" name={`${id}-amount`} checked={choice.kind === "preset" && choice.duffs === p} onChange={() => flow.choose({ kind: "preset", duffs: p })} />
            <span className="topup-choice-amount">{amount(p)}</span>
            {buys(p) && <span className="topup-choice-buys">{buys(p)}</span>}
          </label>
        ))}
        <label className="topup-choice" data-testid="topup-other">
          <input type="radio" name={`${id}-amount`} checked={choice.kind === "other"} onChange={() => flow.choose({ kind: "other", text: "" })} />
          <span className="topup-choice-amount">{TOPUP_COPY.amount.other}</span>
          {otherDuffs !== null && otherProblem === null && buys(otherDuffs) && <span className="topup-choice-buys">{buys(otherDuffs)}</span>}
        </label>
      </fieldset>
      {choice.kind === "other" && (
        <label className="form-field">
          {TOPUP_COPY.amount.field} ({shown.name})
          <input
            data-testid="topup-other-field"
            inputMode="decimal"
            autoFocus
            placeholder={shown.placeholder}
            value={choice.text}
            aria-invalid={otherProblem !== null}
            aria-describedby={`${id}-problem`}
            onChange={(e) => flow.choose({ kind: "other", text: e.target.value })}
          />
        </label>
      )}
      <p className="form-problem" id={`${id}-problem`} role="alert" data-testid="topup-amount-problem">
        {otherProblem}
      </p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="topup-cancel" onClick={() => flow.close()}>
          {TOPUP_COPY.amount.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="topup-next" disabled={duffs === null}>
          {TOPUP_COPY.amount.next}
        </DashButton>
      </div>
    </form>
  );
}

/** T2: the quote, Android's confirm copy, Confirm & pay under the quote's grant. */
function ConfirmStep({ flow, walletId, costs, amount, discreet }: { flow: TopUpFlow; walletId: string; costs: CostTable | undefined; amount: (duffs: number) => string; discreet: boolean }) {
  const quote = useTopUpQuote(walletId, flow.identity, flow.duffs);
  const q = quote.data;
  return (
    <>
      <p className="sheet-text">{TOPUP_COPY.confirm.message}</p>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="topup-quote-error">
          {topUpErrorText(quote.error, amount, discreet)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {TOPUP_COPY.confirm.quoting}
        </p>
      ) : (
        <>
          <dl className="detail-list topup-quote" data-testid="topup-quote">
            <div className="detail-row">
              <dt className="detail-label">{TOPUP_COPY.confirm.amount}</dt>
              <dd className="detail-value">{amount(flow.duffs ?? 0)}</dd>
            </div>
            <div className="detail-row">
              <dt className="detail-label">{TOPUP_COPY.confirm.fee}</dt>
              <dd className="detail-value">{amount(q.feeDuffs)}</dd>
            </div>
            <div className="detail-row topup-total">
              <dt className="detail-label">{TOPUP_COPY.confirm.total}</dt>
              <dd className="detail-value">{amount(q.totalDuffs)}</dd>
            </div>
          </dl>
          {costs && <p className="sheet-text">{TOPUP_COPY.confirm.adds(BigInt(q.credits).toLocaleString("en-US"), estimateText(q.credits, costs))}</p>}
        </>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="topup-back" disabled={flow.running} onClick={() => flow.back()}>
          {TOPUP_COPY.confirm.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="topup-pay" disabled={!q || flow.running} onClick={() => q && void flow.pay(q.grant)}>
          {TOPUP_COPY.confirm.pay}
        </DashButton>
      </div>
    </>
  );
}

export function TopUpSheet({ flow, walletId, network, costs }: { flow: TopUpFlow; walletId: string; network: NetworkInfo; costs: CostTable | undefined }) {
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const amount = (duffs: number) => `${shown.compact(duffs)} ${shown.name}`;
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step } = flow;
  const balance = (credits: string) => creditBalance(credits, costs, discreet, amount, shown.masked());
  const done = step === "done" && flow.outcome ? outcomeText(flow.outcome, balance) : null;
  return (
    <>
      <Sheet title={TOPUP_COPY.title} width="small" testId="topup-sheet" onClose={() => flow.close()}>
        <div data-testid="topup-body" data-step={step} data-outcome={flow.outcome?.state.kind}>
          {step === "amount" && <AmountStep flow={flow} costs={costs} network={network} amount={amount} />}
          {step === "confirm" && <ConfirmStep flow={flow} walletId={walletId} costs={costs} amount={amount} discreet={discreet} />}
          {step === "running" && (
            <div role="status" className="topup-running">
              <p className="topup-title">
                {TOPUP_COPY.running.title}
                <span className="spinner" aria-hidden="true" />
              </p>
              <p className="sheet-text">{TOPUP_COPY.running.text}</p>
            </div>
          )}
          {done && (
            <div role="status">
              <p className="topup-title" data-testid="topup-outcome-title">
                {done.title}
              </p>
              <p className="sheet-text" data-testid="topup-outcome-text">
                {done.text}
              </p>
              <div className="sheet-actions">
                <DashButton variant="filledBlue" size="large" data-testid="topup-done" onClick={() => flow.close()}>
                  {TOPUP_COPY.done}
                </DashButton>
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="topup-refusal">
                {topUpErrorText(flow.error, amount, discreet)}
              </p>
              <div className="sheet-actions">
                {amountCanChange(flow.error) && (
                  <DashButton variant="tintedGray" size="large" data-testid="topup-refused-back" onClick={() => flow.back()}>
                    {TOPUP_COPY.confirm.back}
                  </DashButton>
                )}
                {routesToRepair(flow.error) && (
                  <DashButton
                    variant="tintedBlue"
                    size="large"
                    data-testid="topup-repair"
                    onClick={() => {
                      flow.close();
                      navigate("tools", { toolsTab: "repair" });
                    }}
                  >
                    {TOPUP_COPY.repair}
                  </DashButton>
                )}
                <DashButton variant="filledBlue" size="large" data-testid="topup-close" onClick={() => flow.close()}>
                  {TOPUP_COPY.close}
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
