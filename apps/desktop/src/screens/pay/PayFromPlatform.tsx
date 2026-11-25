// Pay ▸ To an address, given a Platform address (platform/send.ts): the amount (or Max) from the
// Platform balance, the fee estimate and its band, then a confirm sheet that runs the send and
// shows how it ended.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { hasCode } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { PlatformActivity } from "../../api/generated/PlatformActivity";
import type { PlatformSendQuote } from "../../api/generated/PlatformSendQuote";
import type { WithdrawAmount } from "../../api/generated/WithdrawAmount";
import { creditsToDuffs } from "../../api/pending";
import { useBackend, usePlatformActivity, usePlatformAddresses, usePlatformSendQuote } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { addressSendsOff, MAINNET_OFF_COPY } from "../../platform/mainnetOff";
import { hasUnansweredOp } from "../../platform/move";
import { canRetry, heldSend, retryAsks, sendAmount, sendCanRetry, sendErrorText, sendFor, sendOutcomeCopy, sendRefusalText, sendWent, settledBy, staleConfirm, SEND_COPY as C, type PlatformSendFlow, type Settled } from "../../platform/send";
import { GrantSheet } from "../tools/common";
import { FormField, type useAmountField } from "./form";

/** `seen`: the Platform activity when Confirm opened (what a later read is compared with). */
type Confirmed = { quote: PlatformSendQuote; amount: WithdrawAmount; seen: readonly PlatformActivity[] | null | undefined };

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

function ConfirmRows({ q, credits }: { q: PlatformSendQuote; credits: (c: string) => string }) {
  const recipientGets = String(BigInt(q.amountCredits) - BigInt(q.feeCredits));
  const taken = q.feeFromAmount ? q.credits : String(BigInt(q.amountCredits) + BigInt(q.feeCredits));
  return (
    <>
      <dl className="detail-list topup-quote" data-testid="platform-send-confirm">
        <Row label={C.rows.from} testId="platform-send-confirm-from">
          {C.rows.fromValue}
        </Row>
        <Row label={C.rows.to} testId="platform-send-confirm-to" address>
          {q.to}
        </Row>
        <Row label={C.rows.amount} testId="platform-send-confirm-amount">
          {q.feeFromAmount ? C.rows.amountEverything(credits(q.amountCredits)) : <bdi dir="ltr">{credits(q.amountCredits)}</bdi>}
        </Row>
        <Row label={C.rows.fee} testId="platform-send-confirm-fee">
          {C.feeValue(credits(q.feeCredits))}
        </Row>
        {q.feeSource !== null && (
          <Row label={C.rows.feeSource} testId="platform-send-confirm-fee-source" address>
            {q.feeSource}
          </Row>
        )}
        {q.feeFromAmount && (
          <Row label={C.rows.recipientGets} testId="platform-send-confirm-recipient">
            <bdi dir="ltr">{credits(recipientGets)}</bdi>
          </Row>
        )}
        <Row label={q.feeFromAmount ? C.rows.totalExact : C.rows.total} testId="platform-send-confirm-total" total>
          <bdi dir="ltr">{credits(taken)}</bdi>
        </Row>
      </dl>
      <p className="field-hint" data-testid={q.feeFromAmount ? "platform-send-fee-from-amount" : "platform-send-fee-source"}>
        {q.feeFromAmount || q.feeSourceCredits === null ? C.feeFromAmount : C.feeSource(credits(q.feeCredits), credits(q.feeSourceCredits))}
      </p>
      {q.retryOf !== null && (
        <p className="field-hint" data-testid="platform-send-retry-note">
          {C.retryNote}
        </p>
      )}
    </>
  );
}

function SendSheet({
  flow,
  confirmed,
  credits,
  settled,
  activity,
  onClose,
  onRetry,
}: {
  flow: PlatformSendFlow;
  confirmed: Confirmed | null;
  credits: (c: string) => string;
  settled: Settled | null;
  activity: readonly PlatformActivity[] | null | undefined;
  onClose: (sent: boolean) => void;
  onRetry: () => void;
}) {
  const { step, outcome } = flow;
  const q = confirmed?.quote;
  const close = () => onClose(step === "done" && sendWent(outcome, settled));
  // Refused for good: nothing is held, so the payment can be sent again from the form.
  const notSent = settled === "refused" || (outcome?.state === "failed" && !outcome.mayLand);
  return (
    <Sheet title={C.title} width="small" testId="platform-send-sheet" onClose={close}>
      <div data-testid="platform-send-body" data-step={step}>
        {step === "confirm" && q && confirmed && (
          <>
            <p className="sheet-text">{C.confirmText}</p>
            <ConfirmRows q={q} credits={credits} />
            <div className="sheet-actions">
              <DashButton variant="tintedGray" size="large" data-testid="platform-send-cancel" disabled={flow.running} onClick={close}>
                {C.cancel}
              </DashButton>
              <DashButton variant="filledBlue" size="large" data-testid="platform-send-go" disabled={flow.running || flow.isSpent(q.operation)} onClick={() => void flow.send(q)}>
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
              {sendOutcomeCopy(outcome, credits, settled, flow.retryOf !== null).title}
            </p>
            <p className="sheet-text" data-testid="platform-send-outcome-text">
              {sendOutcomeCopy(outcome, credits, settled, flow.retryOf !== null).text}
            </p>
            {flow.noRetry && (
              <p className="form-problem" role="alert" data-testid="platform-send-no-retry">
                {C.noRetry}
              </p>
            )}
            <div className="sheet-actions">
              {!sendWent(outcome, settled) && !notSent && (
                <DashButton variant="tintedGray" size="large" data-testid="platform-send-view" onClick={() => { close(); navigate("activity"); }}>
                  {C.view}
                </DashButton>
              )}
              {canRetry(outcome) && settled === null && (
                <DashButton variant="tintedBlue" size="large" data-testid="platform-send-retry" disabled={flow.running} onClick={onRetry}>
                  {C.retry}
                </DashButton>
              )}
              <DashButton variant="filledBlue" size="large" data-testid="platform-send-done" onClick={close}>
                {notSent ? C.back : C.close}
              </DashButton>
            </div>
          </div>
        )}
        {step === "refused" && (
          <>
            <p className="form-problem" role="alert" data-testid="platform-send-refusal">
              {sendRefusalText(flow.error, credits, activity, flow.retryOf)}
            </p>
            <div className="sheet-actions">
              {sendCanRetry(flow.error) && (
                <DashButton variant="tintedGray" size="large" data-testid="platform-send-refused-back" onClick={() => onClose(false)}>
                  {hasCode(flow.error, "credits.quote_changed") ? C.reviewAgain : C.back}
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

function useCredits(network: NetworkInfo) {
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  return { shown, credits: (c: string) => (discreet ? shown.masked() : `${shown.full(creditsToDuffs(c))} ${shown.name}`) };
}

/** The wallet's send when the form holds no Platform address to show it under (a lock took the page
 * down while it ran, and the form came back empty): its sheet, until it is closed. */
export function HeldPlatformSend({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const flow = useModel(sendFor(b, walletId));
  const { credits } = useCredits(network);
  const activity = usePlatformActivity(walletId);
  const [confirmed, setConfirmed] = useState<Confirmed | null>(null);
  if (confirmed === null && !flow.running && flow.step === "confirm") return <GrantSheet grants={flow.grants} />;
  const retry = async () => {
    const got = await flow.retry("");
    if (got) setConfirmed({ ...got, seen: activity.data });
  };
  const close = () => {
    setConfirmed(null);
    flow.reset();
  };
  const settled = flow.step === "done" ? settledBy(flow.outcome, activity.data) : null;
  return (
    <>
      <SendSheet flow={flow} confirmed={confirmed} credits={credits} settled={settled} activity={activity.data} onClose={close} onRetry={() => void retry()} />
      <GrantSheet grants={flow.grants} />
    </>
  );
}

/** The Platform half of the address form: shown below "Pay to" once it holds a Platform address.
 * `amount` is the form's own field, so what was typed before the address was recognised stays. */
export function PayFromPlatform({ walletId, network, to, amount, onSent }: { walletId: string; network: NetworkInfo; to: string; amount: ReturnType<typeof useAmountField>; onSent: () => void }) {
  const t = useT();
  const b = useBackend();
  const flow = useModel(sendFor(b, walletId));
  const { shown, credits } = useCredits(network);
  const [max, setMax] = useState(false);
  const [confirmed, setConfirmed] = useState<Confirmed | null>(null);
  const balance = usePlatformAddresses(walletId);
  const activity = usePlatformActivity(walletId);
  // Only an unanswered one waits here: a send that may still land is quoted (its exact retry passes; anything else is refused on the form).
  const waiting = hasUnansweredOp(activity.data);
  const [notice, setNotice] = useState<string | null>(null);
  const asked = sendAmount(max, amount.settled ? amount.duffs : undefined);
  // Nothing is quoted behind a result: the send's own events would check its amount against what it left.
  const showing = flow.step === "done";
  // On mainnet this version sends nothing new from Platform: only a held payment's retry is offered.
  const off = addressSendsOff(network, balance.data);
  const quote = usePlatformSendQuote(walletId, to, off || waiting || showing ? null : asked);
  const empty = balance.data === null || (balance.data !== undefined && BigInt(balance.data.credits) === 0n);
  const sheetOpen = confirmed !== null || flow.running || flow.step !== "confirm";

  const q = quote.data;
  const problem = waiting ? C.waiting : quote.isError ? sendErrorText(quote.error, credits, activity.data) : (amount.error ?? null);
  // An operation is single use: after an attempt, Confirm waits for a quote taken since.
  const ready = !waiting && !!q && !quote.isFetching && !quote.isError && asked !== null && !flow.isSpent(q.operation);
  const requote = quote.refetch;
  const handledAttempts = useRef(flow.attempts);
  useEffect(() => {
    if (flow.attempts === handledAttempts.current || showing) return;
    handledAttempts.current = flow.attempts;
    if (!waiting && asked !== null && to !== "") void requote();
  }, [flow.attempts, showing]); // eslint-disable-line react-hooks/exhaustive-deps
  // A Confirm whose quote Activity has overtaken goes back to the form, with a fresh quote.
  useEffect(() => {
    if (!confirmed || flow.step !== "confirm" || flow.running || flow.isSpent(confirmed.quote.operation)) return;
    const stale = staleConfirm(confirmed.quote, confirmed.seen, activity.data);
    if (!stale) return;
    setConfirmed(null);
    setNotice(stale === "went" ? C.went : stale === "gone" ? C.gone : null);
    void requote();
  }, [activity.data, confirmed, flow.step, flow.running, requote]);
  const close = (sent: boolean) => {
    setConfirmed(null);
    flow.reset();
    if (sent) {
      amount.setText("");
      setMax(false);
      onSent();
    }
  };
  // Retry this payment: the same recipient and amount, quoted again (the engine answers the exact retry).
  const retry = async () => {
    const got = await flow.retry(to);
    if (got) setConfirmed({ ...got, seen: activity.data });
  };
  // An earlier payment that may still go through, retried from the form after its sheet is gone.
  const held = heldSend(activity.data);
  const retryHeld = async () => {
    if (!held?.to) return;
    setNotice(null);
    const got = await flow.quoteRetry({ id: held.id, to: held.to }, retryAsks(held));
    if (got) setConfirmed({ ...got, seen: activity.data });
  };
  const settled = flow.step === "done" ? settledBy(flow.outcome, activity.data) : null;
  // A send that went ends the form at once, as Pay's Core forms do.
  const went = flow.step === "done" && sendWent(flow.outcome, settled);
  useEffect(() => {
    if (!went) return;
    amount.setText("");
    setMax(false);
  }, [went]); // eslint-disable-line react-hooks/exhaustive-deps

  const sheet = (
    <>
      {sheetOpen && <SendSheet flow={flow} confirmed={confirmed} credits={credits} settled={settled} activity={activity.data} onClose={close} onRetry={() => void retry()} />}
      <GrantSheet grants={flow.grants} />
    </>
  );
  const earlier = held?.to && (
    <div className="field-hint" data-testid="platform-send-earlier">
      <p>{C.earlier(credits(held.credits), held.to)}</p>
      <DashButton variant="tintedBlue" size="small" data-testid="platform-send-retry-held" disabled={flow.running} onClick={() => void retryHeld()}>
        {C.retryThat}
      </DashButton>
      {flow.noRetry && flow.step === "confirm" && (
        <p className="form-problem" role="alert" data-testid="platform-send-no-retry-form">
          {C.noRetry}
        </p>
      )}
    </div>
  );
  if (off)
    return (
      <>
        <p className="field-hint" role="status" data-testid="platform-send-mainnet-off">
          {MAINNET_OFF_COPY.moveAndSend}
        </p>
        {earlier}
        {sheet}
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
      <FormField label={t("pay.form.amount", { unit: shown.name })} error={amount.error} hint={max ? C.max : balance.data ? C.from(credits(balance.data.credits)) : null}>
        {(p) => (
          <span className="amount-entry">
            <input
              {...p}
              data-testid="pay-amount"
              value={max ? "" : amount.text}
              inputMode="decimal"
              autoComplete="off"
              placeholder={max ? t("pay.form.max") : shown.placeholder}
              onChange={(e) => {
                setNotice(null);
                setMax(false);
                amount.setText(e.target.value);
              }}
            />
            <DashButton variant={max ? "filledBlue" : "tintedBlue"} size="small" data-testid="pay-max" aria-pressed={max} onClick={() => {
                setNotice(null);
                setMax(!max);
              }}>
              {t("pay.form.max")}
            </DashButton>
          </span>
        )}
      </FormField>
      {earlier}
      {notice && (
        <p className="field-hint" role="status" data-testid="platform-send-notice">
          {notice}
        </p>
      )}
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
              {C.fee(credits(q.feeCredits), q.feeFromAmount)}
            </span>
          ) : (
            <span className="field-hint" id="pay-reason" data-testid="pay-incomplete">
              {asked === null ? t("pay.platform.enterAmount") : C.quoting}
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
          onClick={() => {
            if (!q || !asked) return;
            setNotice(null);
            setConfirmed({ quote: q, amount: asked, seen: activity.data });
          }}
        >
          {t("pay.form.review")}
        </DashButton>
      </div>
      {sheet}
    </>
  );
}
