// The withdraw sheet (DASHPAY §4 `CreditsViewModel`, DP6-02): the amount (Max, or an amount in the
// display unit), the destination (the wallet's receive address unless another is pasted), the
// quote's confirm step (credits taken, the network's fee, the Dash expected), progress, the
// outcome, and refusals. The flow is dashpay/withdraw.ts; the sheet can close while the call runs
// and reopen to its result.

import { useId } from "react";
import { useQuery } from "@tanstack/react-query";
import type { CostTable } from "../../api/generated/CostTable";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useReceiveAddress } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { creditBalance } from "../../dashpay/profile";
import {
  amountCanChange,
  refusalDocument,
  unconfirmedIn,
  WITHDRAW_COPY,
  withdrawAmount,
  withdrawalRows,
  withdrawErrorText,
  type WithdrawFlow,
} from "../../dashpay/withdraw";
import { useWithdrawals, useWithdrawQuote } from "../../dashpay/withdrawQueries";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

const C = WITHDRAW_COPY;

/** W1: Max or an amount, and the destination, each checked before Continue; a warning while the
 * last withdrawal's outcome is unknown (D23). */
function FormStep({ flow, walletId, identity, costs, network }: { flow: WithdrawFlow; walletId: string; identity: string; costs: CostTable | undefined; network: NetworkInfo }) {
  const b = useBackend();
  const unconfirmed = flow.unconfirmedFor(identity);
  // My Profile reads the list; the form reads it itself only while one is unconfirmed.
  const recent = useWithdrawals(walletId, identity, unconfirmed, unconfirmed !== null);
  const id = useId();
  const shown = useDisplay(network);
  const receive = useReceiveAddress(walletId);
  const text = flow.text.trim();
  const checked = useDebounced(text, 300);
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, checked], queryFn: () => b.parseAmount(checked, shown.unit), enabled: !flow.max && checked !== "", staleTime: Infinity });
  const to = (flow.to ?? receive.data?.address ?? "").trim();
  const checkedTo = useDebounced(to, 300);
  const cls = useQuery({ queryKey: ["classifyAddress", network.dirName, checkedTo], queryFn: () => b.classifyAddress(network.dirName, checkedTo), enabled: checkedTo !== "", staleTime: Infinity });
  const duffs = !flow.max && checked === text && typeof parsed.data === "number" ? parsed.data : null;
  const amountProblem =
    flow.max || text === "" || checked !== text || parsed.isPending ? null : duffs === null || duffs <= 0 ? C.form.invalidAmount : !costs ? C.form.noCosts : null;
  const toProblem =
    to === "" || checkedTo !== to || cls.isPending ? null : cls.data?.kind === "platform" ? C.form.platformTo : cls.data?.kind !== "core" ? C.form.invalidTo : null;
  const amount = withdrawAmount(flow.max, duffs, costs);
  const ready = amount !== null && amountProblem === null && to !== "" && checkedTo === to && cls.data?.kind === "core";
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) flow.next(amount, to);
      }}
    >
      <p className="sheet-text">{C.form.intro}</p>
      {unconfirmedIn(recent.data, unconfirmed) !== null && (
        <p className="form-problem" role="status" data-testid="withdraw-unconfirmed">
          {C.form.unconfirmed}
        </p>
      )}
      <fieldset className="topup-choices" aria-label={C.form.amount}>
        <label className="topup-choice" data-testid="withdraw-max">
          <input type="radio" name={`${id}-amount`} checked={flow.max} onChange={() => flow.setMax(true)} />
          <span className="topup-choice-amount">{C.form.max}</span>
          <span className="topup-choice-buys">{C.form.maxLine}</span>
        </label>
        <label className="topup-choice" data-testid="withdraw-other">
          <input type="radio" name={`${id}-amount`} checked={!flow.max} onChange={() => flow.setMax(false)} />
          <span className="topup-choice-amount">{C.form.amount}</span>
        </label>
      </fieldset>
      {!flow.max && (
        <label className="form-field">
          {C.form.amount} ({shown.name})
          <input
            data-testid="withdraw-amount"
            inputMode="decimal"
            autoFocus
            placeholder={shown.placeholder}
            value={flow.text}
            aria-invalid={amountProblem !== null}
            aria-describedby={`${id}-amount-problem`}
            onChange={(e) => flow.setText(e.target.value)}
          />
        </label>
      )}
      <p className="form-problem" id={`${id}-amount-problem`} role="alert" data-testid="withdraw-amount-problem">
        {amountProblem}
      </p>
      <label className="form-field">
        {C.form.to}
        <input
          data-testid="withdraw-to"
          spellCheck={false}
          autoComplete="off"
          value={flow.to ?? receive.data?.address ?? ""}
          aria-invalid={toProblem !== null}
          aria-describedby={`${id}-to-hint ${id}-to-problem`}
          onChange={(e) => flow.setTo(e.target.value)}
        />
      </label>
      <p className="field-hint" id={`${id}-to-hint`}>
        {C.form.toHint}
      </p>
      <p className="form-problem" id={`${id}-to-problem`} role="alert" data-testid="withdraw-to-problem">
        {toProblem}
      </p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="withdraw-cancel" onClick={() => flow.close()}>
          {C.form.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="withdraw-next" disabled={!ready}>
          {C.form.next}
        </DashButton>
      </div>
    </form>
  );
}

/** W2: the quote and Confirm & withdraw under its grant. */
function ConfirmStep({ flow, walletId, credits, amount }: { flow: WithdrawFlow; walletId: string; credits: (c: string) => string; amount: (duffs: number) => string }) {
  const quote = useWithdrawQuote(walletId, flow.identity, flow.amount);
  const q = quote.data;
  return (
    <>
      <p className="sheet-text">{C.confirm.message}</p>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="withdraw-quote-error">
          {withdrawErrorText(quote.error, credits)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {C.confirm.quoting}
        </p>
      ) : (
        <dl className="detail-list topup-quote" data-testid="withdraw-quote">
          <div className="detail-row">
            <dt className="detail-label">{C.confirm.withdraw}</dt>
            <dd className="detail-value" data-testid="withdraw-quote-credits">
              {credits(q.credits)}
            </dd>
          </div>
          <div className="detail-row">
            <dt className="detail-label">{C.confirm.fee}</dt>
            <dd className="detail-value" data-testid="withdraw-quote-fee">
              {credits(q.feeCredits)}
            </dd>
          </div>
          <div className="detail-row topup-total">
            <dt className="detail-label">{C.confirm.total}</dt>
            <dd className="detail-value">{credits(String(BigInt(q.credits) + BigInt(q.feeCredits)))}</dd>
          </div>
          <div className="detail-row">
            <dt className="detail-label">{C.confirm.receive}</dt>
            <dd className="detail-value" data-testid="withdraw-quote-duffs">
              {amount(q.expectedDuffs)}
            </dd>
          </div>
          <div className="detail-row">
            <dt className="detail-label">{C.confirm.to}</dt>
            <dd className="detail-value withdraw-to" data-testid="withdraw-quote-to">
              {flow.destination}
            </dd>
          </div>
        </dl>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="withdraw-back" disabled={flow.running} onClick={() => flow.back()}>
          {C.confirm.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="withdraw-go" disabled={!q || quote.isError || quote.isFetching || flow.running} onClick={() => q && void flow.withdraw(q.grant)}>
          {C.confirm.go}
        </DashButton>
      </div>
    </>
  );
}

export function WithdrawSheet({ flow, walletId, network, costs }: { flow: WithdrawFlow; walletId: string; network: NetworkInfo; costs: CostTable | undefined }) {
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const amount = (duffs: number) => (discreet ? shown.masked() : `${shown.compact(duffs)} ${shown.name}`);
  // A credit figure as Dash at the table's rate, else in credits (as the credits block).
  const credits = (c: string) => creditBalance(c, costs, discreet, amount, shown.masked());
  const { step, outcome } = flow;
  const named = refusalDocument(flow.error);
  return (
    <>
      <Sheet title={C.title} width="small" testId="withdraw-sheet" onClose={() => flow.close()}>
        <div data-testid="withdraw-body" data-step={step}>
          {step === "form" && flow.identity !== null && <FormStep flow={flow} walletId={walletId} identity={flow.identity} costs={costs} network={network} />}
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
              <p className="topup-title" data-testid="withdraw-outcome-title">
                {C.done.title}
              </p>
              <p className="sheet-text" data-testid="withdraw-outcome-text">
                {C.done.text(outcome.expectedDuffs === null ? credits(outcome.credits) : amount(outcome.expectedDuffs), flow.destination ?? "")}{" "}
                {outcome.remainingCredits === null ? C.done.landed : C.done.balance(credits(outcome.remainingCredits))}
              </p>
              <div className="sheet-actions">
                <DashButton variant="filledBlue" size="large" data-testid="withdraw-done" onClick={() => flow.close()}>
                  {C.ok}
                </DashButton>
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="withdraw-refusal">
                {withdrawErrorText(flow.error, credits)}
              </p>
              {named !== null && (
                <p className="sheet-text" data-testid="withdraw-document">
                  {C.refused.document} <span className="withdraw-to">{named}</span>
                </p>
              )}
              <div className="sheet-actions">
                {amountCanChange(flow.error) && (
                  <DashButton variant="tintedGray" size="large" data-testid="withdraw-refused-back" onClick={() => flow.back()}>
                    {C.confirm.back}
                  </DashButton>
                )}
                <DashButton variant="filledBlue" size="large" data-testid="withdraw-close" onClick={() => flow.close()}>
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

/** My Profile's Withdraw (D9): beside Top up; nothing to take while the balance is unknown or 0. */
export function WithdrawButton({ flow, identity, balance }: { flow: WithdrawFlow; identity: string; balance: string | null }) {
  const empty = balance === null || BigInt(balance) === 0n;
  return (
    <DashButton variant="tintedBlue" data-testid="profile-withdraw" disabled={empty || flow.running} onClick={() => flow.begin(identity)}>
      {flow.running ? C.running.title : C.action}
    </DashButton>
  );
}

/** My Profile's recent withdrawals (D11) and a withdrawal that ended while the sheet was hidden. */
export function RecentWithdrawals({ flow, walletId, identity, costs, amount, date }: {
  flow: WithdrawFlow;
  walletId: string;
  identity: string;
  costs: CostTable | undefined;
  amount: (duffs: number) => string;
  date: (secs: number) => string;
}) {
  const recent = useWithdrawals(walletId, identity, flow.unconfirmedFor(identity));
  const rows = withdrawalRows(recent.data, costs, amount);
  const unconfirmed = unconfirmedIn(recent.data, flow.unconfirmedFor(identity));
  return (
    <>
      {unconfirmed !== null && (
        <div className="profile-row" data-testid="profile-withdraw-unconfirmed" data-pending>
          <div>
            <p className="profile-row-title">{C.recent.unconfirmed}</p>
            <p className="profile-credits-line">
              {C.recent.unconfirmedText} <span className="withdraw-to">{unconfirmed}</span>
            </p>
          </div>
        </div>
      )}
      {flow.unseenFor(identity) && (
        <div className="profile-row" data-testid="profile-withdraw-unseen">
          <p className="profile-row-title" role="status">
            {C.unseen}
          </p>
          <div className="profile-row-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="profile-withdraw-show" onClick={() => flow.begin(identity)}>
              {C.show}
            </DashButton>
          </div>
        </div>
      )}
      {rows.map((r) => (
        <div key={r.id} className="profile-row" data-testid="profile-withdrawal" data-pending={r.pending}>
          <div>
            <p className="profile-row-title">{r.title}</p>
            <p className="profile-credits-line">
              {r.status}
              {r.at !== null && ` · ${date(r.at)}`}
            </p>
          </div>
        </div>
      ))}
    </>
  );
}
