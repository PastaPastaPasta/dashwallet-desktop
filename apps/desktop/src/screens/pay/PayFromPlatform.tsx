// Pay ▸ To an address, given a Platform address (platform/send.ts): the amount (or Max) from the
// Platform balance, the fee estimate and its band, then a confirm sheet that runs the send and
// shows how it ended.

import { useState, type ReactNode } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { PlatformSendOutcome } from "../../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../../api/generated/PlatformSendQuote";
import type { WithdrawAmount } from "../../api/generated/WithdrawAmount";
import { creditsToDuffs } from "../../api/pending";
import { useBackend, usePlatformActivity, usePlatformAddresses, usePlatformSendQuote } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { hasPendingMove } from "../../platform/move";
import { sendAmount, sendCanRetry, sendErrorText, sendFor, SEND_COPY as C, type PlatformSendFlow } from "../../platform/send";
import { GrantSheet } from "../tools/common";
import { FormField, type useAmountField } from "./form";

type Confirmed = { quote: PlatformSendQuote; amount: WithdrawAmount };

function Row({ label, testId, children, total, address }: { label: string; testId: string; children: ReactNode; total?: boolean; address?: boolean }) {
  return (
    <div className={total ? "detail-row topup-total" : "detail-row"}>
      <dt className="detail-label">{label}</dt>
      <dd className={address ? "detail-value withdraw-to" : "detail-value"} data-testid={testId}>
        {children}
      </dd>
    </div>
  );
}

function outcomeCopy(outcome: PlatformSendOutcome, credits: (c: string) => string): { title: string; text: string } {
  switch (outcome.state) {
    case "done":
      return { title: C.done.title, text: C.done.text(credits(outcome.credits)) };
    case "pending":
      return C.pending;
    case "failed":
      return C.failed;
    case "replaced":
      return C.replaced;
  }
}

function SendSheet({ flow, confirmed, credits, onClose }: { flow: PlatformSendFlow; confirmed: Confirmed | null; credits: (c: string) => string; onClose: (sent: boolean) => void }) {
  const { step, outcome } = flow;
  const q = confirmed?.quote;
  const close = () => onClose(step === "done");
  return (
    <Sheet title={C.title} width="small" testId="platform-send-sheet" onClose={close}>
      <div data-testid="platform-send-body" data-step={step}>
        {step === "confirm" && q && confirmed && (
          <>
            <p className="sheet-text">{C.confirmText}</p>
            <dl className="detail-list topup-quote" data-testid="platform-send-confirm">
              <Row label={C.rows.from} testId="platform-send-confirm-from">
                {C.rows.fromValue}
              </Row>
              <Row label={C.rows.to} testId="platform-send-confirm-to" address>
                {q.to}
              </Row>
              <Row label={C.rows.amount} testId="platform-send-confirm-amount">
                {credits(q.amountCredits)}
              </Row>
              <Row label={C.rows.fee} testId="platform-send-confirm-fee">
                {C.feeValue(credits(q.feeCredits), credits(q.maxFeeCredits))}
              </Row>
              <Row label={C.rows.total} testId="platform-send-confirm-total" total>
                {credits(String(BigInt(q.amountCredits) + (q.feeFromAmount ? 0n : BigInt(q.feeCredits))))}
              </Row>
            </dl>
            {q.feeFromAmount && (
              <p className="field-hint" data-testid="platform-send-fee-from-amount">
                {C.feeFromAmount}
              </p>
            )}
            <div className="sheet-actions">
              <DashButton variant="tintedGray" size="large" data-testid="platform-send-cancel" disabled={flow.running} onClick={close}>
                {C.cancel}
              </DashButton>
              <DashButton variant="filledBlue" size="large" data-testid="platform-send-go" disabled={flow.running} onClick={() => void flow.send(q.grant, q.to, confirmed.amount)}>
                {C.go}
              </DashButton>
            </div>
          </>
        )}
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
            <p className="topup-title" data-testid="platform-send-outcome-title" data-state={outcome.state}>
              {outcomeCopy(outcome, credits).title}
            </p>
            <p className="sheet-text" data-testid="platform-send-outcome-text">
              {outcomeCopy(outcome, credits).text}
            </p>
            <div className="sheet-actions">
              {outcome.state !== "done" && (
                <DashButton variant="tintedGray" size="large" data-testid="platform-send-view" onClick={() => { close(); navigate("activity"); }}>
                  {C.view}
                </DashButton>
              )}
              <DashButton variant="filledBlue" size="large" data-testid="platform-send-done" onClick={close}>
                {C.close}
              </DashButton>
            </div>
          </div>
        )}
        {step === "refused" && (
          <>
            <p className="form-problem" role="alert" data-testid="platform-send-refusal">
              {sendErrorText(flow.error, credits)}
            </p>
            <div className="sheet-actions">
              {sendCanRetry(flow.error) && (
                <DashButton variant="tintedGray" size="large" data-testid="platform-send-refused-back" onClick={() => onClose(false)}>
                  {C.back}
                </DashButton>
              )}
              <DashButton variant="filledBlue" size="large" data-testid="platform-send-close" onClick={close}>
                {C.close}
              </DashButton>
            </div>
          </>
        )}
      </div>
    </Sheet>
  );
}

/** The Platform half of the address form: shown below "Pay to" once it holds a Platform address.
 * `amount` is the form's own field, so what was typed before the address was recognised stays. */
export function PayFromPlatform({ walletId, network, to, amount, onSent }: { walletId: string; network: NetworkInfo; to: string; amount: ReturnType<typeof useAmountField>; onSent: () => void }) {
  const b = useBackend();
  const flow = useModel(sendFor(b, walletId));
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const credits = (c: string) => (discreet ? shown.masked() : `${shown.full(creditsToDuffs(c))} ${shown.name}`);
  const [max, setMax] = useState(false);
  const [confirmed, setConfirmed] = useState<Confirmed | null>(null);
  const balance = usePlatformAddresses(walletId);
  const activity = usePlatformActivity(walletId);
  const waiting = hasPendingMove(activity.data);
  const asked = sendAmount(max, amount.settled ? amount.duffs : undefined);
  const quote = usePlatformSendQuote(walletId, to, waiting ? null : asked);
  const empty = balance.data === null || (balance.data !== undefined && BigInt(balance.data.credits) === 0n);
  const sheetOpen = confirmed !== null || flow.running || flow.step !== "confirm";

  const q = quote.data;
  const problem = waiting ? C.waiting : quote.isError ? sendErrorText(quote.error, credits) : (amount.error ?? null);
  const ready = !waiting && !!q && !quote.isFetching && asked !== null;
  const close = (sent: boolean) => {
    setConfirmed(null);
    flow.reset();
    if (sent) {
      amount.setText("");
      setMax(false);
      onSent();
    }
  };

  const sheet = (
    <>
      {sheetOpen && <SendSheet flow={flow} confirmed={confirmed} credits={credits} onClose={close} />}
      <GrantSheet grants={flow.grants} />
    </>
  );
  // A send that emptied the balance keeps its result sheet up.
  if (empty)
    return (
      <>
        <p className="form-problem" role="alert" data-testid="platform-send-empty">
          {C.empty}
        </p>
        {sheet}
      </>
    );

  return (
    <>
      <p className="placeholder-note" data-testid="platform-send-intro">
        {C.intro}
      </p>
      <FormField label={`Amount (${shown.name})`} error={amount.error} hint={max ? C.max : balance.data ? C.from(credits(balance.data.credits)) : null}>
        {(p) => (
          <span className="amount-entry">
            <input
              {...p}
              data-testid="pay-amount"
              value={max ? "" : amount.text}
              inputMode="decimal"
              autoComplete="off"
              placeholder={max ? "Max" : shown.placeholder}
              onChange={(e) => {
                setMax(false);
                amount.setText(e.target.value);
              }}
            />
            <DashButton variant={max ? "filledBlue" : "tintedBlue"} size="small" data-testid="pay-max" aria-pressed={max} onClick={() => setMax(!max)}>
              Max
            </DashButton>
          </span>
        )}
      </FormField>
      <div className="pay-footer" data-testid="pay-footer">
        <div className="pay-footer-text" aria-live="polite">
          {problem ? (
            <span className="form-problem" id="pay-reason" data-testid="pay-problem">
              {problem}
              {waiting && (
                <>
                  {" "}
                  <DashButton variant="plainBlue" size="small" data-testid="platform-send-waiting-view" onClick={() => navigate("activity")}>
                    {C.view}
                  </DashButton>
                </>
              )}
            </span>
          ) : q ? (
            <span className="pay-fee-line" id="pay-reason" data-testid="pay-fee-line">
              {C.fee(credits(q.feeCredits), credits(q.maxFeeCredits))}
            </span>
          ) : (
            <span className="field-hint" id="pay-reason" data-testid="pay-incomplete">
              {asked === null ? "Enter an amount first." : C.quoting}
            </span>
          )}
        </div>
        <DashButton
          type="button"
          variant="filledBlue"
          size="large"
          data-testid="pay-continue"
          disabled={!ready}
          why={problem ?? C.quoting}
          whyId="pay-reason"
          onClick={() => q && asked && setConfirmed({ quote: q, amount: asked })}
        >
          Review
        </DashButton>
      </div>
      {sheet}
    </>
  );
}
