// Pay ▸ To a contact: the "From" step and a payment from the Shielded or Platform balance
// (C-funding D-C5, fromBalance.ts). The amount typed is what leaves the balance; the confirm says
// about what the contact receives and the fee (P13). No Max, no "subtract fee", no coin control.

import { useEffect, useId, useState, type Ref } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ContactPayQuoteInfo } from "../../api/generated/ContactPayQuoteInfo";
import type { PlatformActivity } from "../../api/generated/PlatformActivity";
import { CREDITS_PER_DUFF, creditsToDuffs } from "../../api/pending";
import { useBackend } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import type { Display as Shown } from "../../format/display";
import { FiatSecondary, FiatToggle } from "../../fiat/entry";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";
import { FormField, type usePayAmount } from "./form";
import { contactPayFor, fromErrorText, outcomeText, sourceLabel, totalText, type ContactPayFlow, type FromRow, type PaySource } from "./fromBalance";

/** The sources, each with its balance and, when it cannot pay, its reason (P16). */
export function FromStep({ rows, source, onChange }: { rows: readonly FromRow[]; source: PaySource; onChange: (s: PaySource) => void }) {
  const t = useT();
  const id = useId();
  return (
    <fieldset className="topup-choices pay-with-sources" data-testid="pay-from" aria-labelledby={`${id}-title`}>
      <legend className="field-label" id={`${id}-title`}>
        {t("pay.contact.from.title")}
      </legend>
      {rows.map((r) => (
        <label key={r.source} className="topup-choice pay-with-source" data-testid={`pay-from-${r.source}`} data-disabled={r.reason !== null || undefined}>
          <input
            type="radio"
            name={`${id}-source`}
            checked={source === r.source}
            disabled={r.reason !== null}
            aria-describedby={r.reason !== null ? `${id}-${r.source}-reason` : undefined}
            onChange={() => onChange(r.source)}
          />
          <span className="topup-choice-amount">{r.label}</span>
          {r.balance !== null && (
            <span className="topup-choice-buys" data-testid={`pay-from-${r.source}-balance`}>
              <bdi dir="ltr">{r.balance}</bdi>
            </span>
          )}
          {r.reason !== null && (
            <span className="topup-choice-buys pay-with-reason" id={`${id}-${r.source}-reason`} data-testid={`pay-from-${r.source}-reason`}>
              {r.reason}
            </span>
          )}
        </label>
      ))}
    </fieldset>
  );
}

/** The amount from a balance: what leaves it (P13), with no Max and no "subtract fee". */
export function BalanceAmount({ shown, amount, inputRef }: { shown: Shown; amount: ReturnType<typeof usePayAmount>; inputRef?: Ref<HTMLInputElement> }) {
  const t = useT();
  const { fiat } = amount;
  return (
    <FormField label={t("pay.form.amount", { unit: fiat.unitName })} error={fiat.error ?? amount.error} hint={fiat.toggle.shown ? <FiatSecondary entry={fiat} duffs={amount.duffs} testId="pay-amount-other" /> : undefined}>
      {(p) => (
        <span className="amount-entry">
          <input {...p} ref={inputRef} data-testid="pay-amount" value={fiat.value} inputMode="decimal" autoComplete="off" placeholder={fiat.placeholder} onChange={(e) => fiat.onChange(e.target.value)} />
          <FiatToggle entry={fiat} shown={shown} testId="pay-amount-toggle" />
        </span>
      )}
    </FormField>
  );
}

/** Amounts in duffs and credits with the unit, masked in discreet mode. */
function useMaskedAmounts(shown: Shown) {
  const discreet = useUi((s) => s.discreet);
  const amount = (duffs: number) => (discreet ? shown.masked() : shown.withUnit(duffs));
  return { amount, credit: (c: string) => amount(creditsToDuffs(c)) };
}

/** A payment from a balance in the contact form: its quote while the amount is settled, what holds
 * Review back, and the sheet once Review opened it. */
export function useBalancePay({
  walletId,
  identity,
  contact,
  source,
  duffs,
  settled,
  shown,
  blocked,
  basis,
}: {
  walletId: string;
  identity: string;
  contact: string;
  source: PaySource;
  duffs: number | null | undefined;
  settled: boolean;
  shown: Shown;
  /** Why the chosen source cannot pay now: Review waits with it, and nothing is quoted. */
  blocked: string | null;
  /** What the quote depends on besides the amount (the balance, each read of Platform activity): a
   * change reads a new quote for the form. An open confirmation keeps the quote it showed. */
  basis: string;
}) {
  const b = useBackend();
  const client = useQueryClient();
  const { credit } = useMaskedAmounts(shown);
  const flow = useModel(contactPayFor(b, walletId, identity, contact));
  const [confirmed, setConfirmed] = useState<ContactPayQuoteInfo | null>(null);
  const active = source !== "core";
  const credits = active && blocked === null && settled && typeof duffs === "number" && duffs > 0 ? String(BigInt(duffs) * CREDITS_PER_DUFF) : null;
  const open = confirmed !== null || flow.step !== "confirm" || flow.running;
  const quote = useQuery({
    queryKey: ["contactPayQuote", walletId, identity, contact, source, credits, basis] as const,
    queryFn: () => b.contactPayQuote(walletId, identity, contact, credits!, source),
    enabled: credits !== null && !open,
    retry: false,
  });
  const problem = !active ? null : (blocked ?? (credits !== null && quote.isError ? fromErrorText(quote.error, credits, credit) : null));
  const q = quote.data && quote.data.credits === credits && quote.data.source === source ? quote.data : null;
  return {
    active,
    flow,
    confirmed,
    problem,
    incomplete: credits === null && blocked === null,
    ready: q !== null && !quote.isFetching && problem === null,
    fee: q ? creditsToDuffs(q.feeCredits) : null,
    review: () => {
      if (q) setConfirmed(q);
    },
    close: () => {
      setConfirmed(null);
      flow.reset();
      // After an attempt the form quotes again: a refused grant or a settled hold changes it.
      void client.invalidateQueries({ queryKey: ["contactPayQuote", walletId, identity, contact] });
    },
  };
}

/** Confirm, then how the payment ended (the D-C5 Outcomes and Copy tables). */
export function BalancePaySheet({
  flow,
  quote,
  name,
  sub,
  note,
  shown,
  before,
  onClose,
}: {
  flow: ContactPayFlow;
  quote: ContactPayQuoteInfo | null;
  /** The Platform activity as last read: a lost reply is settled against what is new since. */
  before: readonly PlatformActivity[] | null | undefined;
  name: string;
  /** What tells two contacts of one name apart: the `@username`, or the identity's short id. */
  sub: string;
  note: string | null;
  shown: Shown;
  /** `paid`: the payment went or may have, so the form clears. */
  onClose: (paid: boolean) => void;
}) {
  const t = useT();
  const { amount, credit } = useMaskedAmounts(shown);
  const { step, outcome } = flow;
  // A prompt still open when the sheet goes (a lock) is cancelled: the flow is not left running.
  useEffect(() => () => flow.grants.cancel(), [flow]);
  const close = () => onClose((step === "done" && outcome !== null && outcome.state !== "notSent") || step === "lost");
  const state = step === "lost" ? "pending" : outcome?.state;
  const text = state ? outcomeText(state, name) : null;
  return (
    <>
      <Sheet title={t("pay.contact.pay", { name })} width="small" testId="pay-balance-sheet" closable={!flow.running} onClose={close}>
        <div data-testid="pay-balance-body" data-step={step}>
          {step === "confirm" && quote && (
            <>
              <p className="pay-payee-names" data-testid="pay-balance-payee">
                <span className="pay-payee-name">{name}</span>
                <span className="contact-row-username">{sub}</span>
              </p>
              <p className="sheet-text" data-testid="pay-balance-confirm">
                {t("pay.contact.from.confirm", { name, amount: credit(quote.credits), source: sourceLabel(quote.source), received: amount(quote.expectedDuffs), fee: credit(quote.feeCredits) })}
              </p>
              <p className="sheet-text" data-testid="pay-balance-total">
                {totalText(quote, name, credit)}
              </p>
              <div className="sheet-actions">
                <DashButton variant="tintedGray" size="large" data-testid="pay-balance-cancel" disabled={flow.running} onClick={() => onClose(false)}>
                  {t("common.cancel")}
                </DashButton>
                <DashButton variant="filledBlue" size="large" data-testid="pay-balance-go" disabled={flow.running} onClick={() => void flow.pay(quote, note, before)}>
                  {t("pay.confirm.send")}
                </DashButton>
              </div>
            </>
          )}
          {step === "running" && (
            <div role="status" className="topup-running">
              <p className="topup-title">
                {t("pay.result.sending")}
                <span className="spinner" aria-hidden="true" />
              </p>
            </div>
          )}
          {(step === "done" || step === "lost") && state && text && (
            <div role="status">
              <p className="topup-title" data-testid="pay-balance-outcome-title" data-state={step === "lost" ? "lost" : state}>
                {text.title}
              </p>
              <p className="sheet-text" data-testid="pay-balance-outcome-text">
                {text.text}
              </p>
              <div className="sheet-actions">
                {state !== "notSent" && (
                  <DashButton
                    variant="tintedGray"
                    size="large"
                    data-testid="pay-balance-show"
                    onClick={() => {
                      close();
                      navigate("activity");
                    }}
                  >
                    {t("pay.result.show")}
                  </DashButton>
                )}
                <DashButton variant="filledBlue" size="large" data-testid="pay-balance-done" onClick={close}>
                  {state === "notSent" ? t("pay.result.back") : t("pay.result.close")}
                </DashButton>
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="pay-balance-refusal">
                {fromErrorText(flow.error, quote?.credits ?? null, credit)}
              </p>
              <p className="sheet-text">{t("pay.contact.nothingPaid")}</p>
              <div className="sheet-actions">
                <DashButton variant="filledBlue" size="large" data-testid="pay-balance-close" onClick={() => onClose(false)}>
                  {t("pay.result.close")}
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
