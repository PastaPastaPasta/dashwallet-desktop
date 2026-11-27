// The Pay form (U-03; DP3-06 D8): a labelled field, the amount as typed and as the
// engine parses it, the live estimate of the draft and the refusal it brings, Max, and the footer
// with the fee line and Review.

import { useEffect, useId, useRef, useState, useSyncExternalStore, type ReactNode, type Ref } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import { keys, useBackend } from "../../api/queries";
import { navigate, ui } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { AmountText, DashButton } from "../../components/basics";
import { FiatSecondary, FiatToggle, useFiatEntry } from "../../fiat/entry";
import type { useDisplay } from "../../format/display";
import { useT, t } from "../../i18n";
import { availableLabel, changePolicy, coinSource, DEFAULT_ADVANCED, estimateRefusal, feeMode, incompleteText, isContact, mixedFeeText, sendErrorText, withSubtractFee, type Advanced, type CoinChoice, type Confirmed, type Outcome, type PayForm, type PayNotice, type PayRecipient, type Sending } from "./model";
import type { Recipient } from "../../api/generated/Recipient";
import type { DraftApi, DraftSpec } from "./useDraft";

/** How long a field stays unchanged before it is checked and the draft estimated (U-03 D5). */
export const SETTLE_MS = 300;

type Shown = ReturnType<typeof useDisplay>;

/** Where a form is: editing, confirming, sending (the payment outlived the confirm sheet), or
 * showing the result. */
export type Phase =
  | { kind: "editing" }
  | { kind: "confirm"; confirmed: Confirmed; spec: DraftSpec; unsigned: boolean }
  | { kind: "sending"; sending: Sending }
  | { kind: "result"; outcome: Outcome; retrying: boolean };

/** One press of Send, from before its first call until its result sheet closes: signing
 * (`preparing`), handed to the broadcast (`sending`), then its result, with a Try again of it
 * running (`retrying`), or a notice on the form in place of a result, kept until it is dismissed. */
type Kept =
  | { kind: "preparing" | "sending"; attempt: number; sending: Sending }
  | { kind: "result"; attempt: number; outcome: Outcome; retrying: boolean }
  | { kind: "notice"; attempt: number; notice: PayNotice };

/** Each form's payment by `payFormKey`: a lock takes the page down at once, and the unlock shows
 * the same payment, not a form that looks unsent. Only the attempt kept under a key may change
 * it, so an older payment's late answer never replaces a newer one's. */
const kept = new Map<string, Kept>();
const keptListeners = new Set<() => void>();
const subscribeKept = (l: () => void) => {
  keptListeners.add(l);
  return () => void keptListeners.delete(l);
};
function keep(key: string, k: Kept | null) {
  if (k) kept.set(key, k);
  else kept.delete(key);
  keptListeners.forEach((l) => l());
}
let attempts = 0;

/** Tests: what the form under `key` keeps. */
export const keptPayment = (key: string): Kept | null => kept.get(key) ?? null;

/** Tests: nothing kept. */
export const resetKeptPayments = () => kept.clear();

export const payFormKey = (walletId: string, contact?: string) => (contact === undefined ? `${walletId}:address` : `${walletId}:contact:${contact}`);

/** What an outcome says once its draft closed: it has no retry. */
export const outliving = (o: Outcome): Outcome => (o.kind === "unknown" && o.retry ? { ...o, retry: false } : o);

/** A payment's own way to report, ignored once another payment took its form's place. */
export type PayAttempt = {
  /** Handed to the broadcast: it may reach the network from here. */
  broadcasting: (sending: Sending) => void;
  outcome: (o: Outcome) => void;
  /** Refused before anything was signed (the passphrase): the confirm sheet stays. */
  release: () => void;
  /** Refused before anything was signed, said by a notice on the form: the confirm sheet goes. */
  notice: (n: PayNotice) => void;
};

/** `attempt`'s handle on the payment kept under `key`. `live()`: its draft is still the page's
 * own, so an unknown result keeps its retry. */
export function payAttempt(key: string, attempt: number, live: () => boolean, onSettled: (o: Outcome | null) => void = () => {}): PayAttempt {
  const mine = () => kept.get(key)?.attempt === attempt;
  return {
    broadcasting: (sending) => {
      if (mine()) keep(key, { kind: "sending", attempt, sending });
    },
    outcome: (o) => {
      if (!mine()) return;
      onSettled(o);
      keep(key, { kind: "result", attempt, outcome: live() ? o : outliving(o), retrying: false });
    },
    release: () => {
      if (mine() && kept.get(key)?.kind === "preparing") keep(key, null);
    },
    notice: (notice) => {
      if (!mine() || kept.get(key)?.kind !== "preparing") return;
      onSettled(null);
      keep(key, { kind: "notice", attempt, notice });
    },
  };
}

/** The notice kept under `key` was read: gone, as a new payment would replace it. */
export function dismissNotice(key: string) {
  if (kept.get(key)?.kind === "notice") keep(key, null);
}

/** Starts a payment under `key`, in place of whatever was kept there. */
export function beginPayment(key: string, sending: Sending): number {
  const attempt = ++attempts;
  keep(key, { kind: "preparing", attempt, sending });
  return attempt;
}

/** Starts a Try again of the unknown result kept under `key`: its attempt, or `null` for none. */
export function beginRetry(key: string): number | null {
  const k = kept.get(key);
  if (k?.kind !== "result" || k.retrying || k.outcome.kind !== "unknown" || !k.outcome.retry) return null;
  keep(key, { ...k, retrying: true });
  return k.attempt;
}

/** The draft under `key` closed: an unknown result kept there loses its retry. */
export function draftClosed(key: string) {
  const k = kept.get(key);
  if (k?.kind === "result" && k.outcome.kind === "unknown" && k.outcome.retry) keep(key, { ...k, outcome: outliving(k.outcome) });
}

/** The result sheet closed on `attempt`'s result: what it was, or `null` when it is not kept (or
 * its Try again still runs). */
export function acknowledge(key: string, attempt: number): Outcome | null {
  const k = kept.get(key);
  if (k?.kind !== "result" || k.attempt !== attempt || k.retrying) return null;
  keep(key, null);
  return k.outcome;
}

/** A Pay form's sheets (U-03): Review's confirm, then the result. While either is up the form
 * estimates nothing (`held`), so the payment's own events re-check nothing behind them. A payment
 * that went, or may have, ends the form at once (`clear`); its draft stays open until the result
 * sheet closes, as unknown's Try again rebroadcasts through it. A failed one leaves the form as it was.
 * From Send the payment is kept under `key` (`payFormKey`), so a remounted form shows it;
 * `filledBy: "link"`, the form Pay's prefill and coins fill, which the broadcast spends. */
export function usePaySheets(key: string, walletId: string, draft: DraftApi, clear: () => void, filledBy: "link" | null = null) {
  const client = useQueryClient();
  const [confirm, setConfirm] = useState<Extract<Phase, { kind: "confirm" }> | null>(null);
  const stored = useSyncExternalStore(subscribeKept, () => kept.get(key) ?? null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // The page's draft closes with it.
      draftClosed(key);
    };
  }, [key]);
  const live = () => mounted.current;
  // `null`: a notice, which leaves the form as it was.
  const settled = (o: Outcome | null) => {
    if (o && o.kind !== "failed") clear();
    if (mounted.current) setConfirm(null);
  };
  const phase: Phase =
    stored?.kind === "result"
      ? { kind: "result", outcome: stored.outcome, retrying: stored.retrying }
      : (confirm ?? (stored && stored.kind !== "notice" ? { kind: "sending", sending: stored.sending } : { kind: "editing" }));
  return {
    phase,
    /** What the last payment's refusal says on the form, until `dismissNotice` or another payment. */
    notice: stored?.kind === "notice" ? stored.notice : null,
    dismissNotice: () => dismissNotice(key),
    held: phase.kind === "confirm" || phase.kind === "sending" || (phase.kind === "result" && phase.outcome.kind !== "failed"),
    review: (confirmed: Confirmed, spec: DraftSpec, unsigned = false) => setConfirm({ kind: "confirm", confirmed, spec, unsigned }),
    cancel: () => setConfirm(null),
    /** Send was pressed, `sending` the payment as the review showed it. */
    begin: (sending: Sending): PayAttempt => {
      const attempt = payAttempt(key, beginPayment(key, sending), live, settled);
      return {
        ...attempt,
        // From here the payment may reach the network: the link or the coins that filled the form
        // are spent on it, so a form mounted again (after a lock) starts empty rather than offering it again.
        broadcasting: (s) => {
          attempt.broadcasting(s);
          if (filledBy === "link") ui.set({ payPrefill: null, payCoins: null });
        },
      };
    },
    /** A result that is no payment's (Create Unsigned's "changed"). */
    outcome: (o: Outcome) => payAttempt(key, beginPayment(key, { amount: 0, fee: 0 }), live, settled).outcome(o),
    retry: (): ((o: Outcome) => void) | null => {
      const attempt = beginRetry(key);
      return attempt === null ? null : payAttempt(key, attempt, live).outcome;
    },
    done: (show: boolean) => {
      if (stored?.kind !== "result") return;
      const o = acknowledge(key, stored.attempt);
      if (!o) return;
      if (o.kind !== "failed") {
        void draft.close();
        void client.invalidateQueries({ queryKey: keys.coinsAll(walletId) });
      }
      if (show && o.kind === "sent") navigate("activity", { detail: o.txid });
    },
  };
}

export function FormField({
  label,
  error,
  hint,
  children,
}: {
  label: string;
  error?: string | null;
  hint?: ReactNode;
  children: (props: { id: string; "aria-invalid": boolean; "aria-describedby": string | undefined }) => ReactNode;
}) {
  const id = useId();
  const described = error ? `${id}-error` : hint ? `${id}-hint` : undefined;
  return (
    <div className="form-field">
      <label htmlFor={id}>{label}</label>
      {children({ id, "aria-invalid": !!error, "aria-describedby": described })}
      {error ? (
        <span id={`${id}-error`} className="field-error">
          {error}
        </span>
      ) : (
        hint && (
          <span id={`${id}-hint`} className="field-hint">
            {hint}
          </span>
        )
      )}
    </div>
  );
}

/** The draft paying `recipient` (and the added `more`) with the Advanced choices; `null` while
 * any is incomplete (`more` is `null` while an added recipient is). */
export function draftSpec(recipient: PayRecipient | null, a: Advanced, more: Recipient[] | null = []): DraftSpec | null {
  const fee = feeMode(a);
  const change = changePolicy(a);
  const noCoins = a.coins === "selected" && a.selected.length === 0;
  return recipient && more && fee && change && !noCoins ? { recipient, more, source: coinSource(a), fee, change } : null;
}

/** The amount field: the text, and once it settled the engine's parse of it (`undefined` while
 * empty or unread, `null` when it is not an amount). */
export function useAmountField(shown: Shown, initial: string) {
  const b = useBackend();
  const [text, setText] = useState(initial);
  const typed = text.trim();
  const checked = useDebounced(typed, SETTLE_MS);
  const parsed = useQuery({
    queryKey: ["parseAmount", shown.unit, checked],
    queryFn: () => b.parseAmount(checked, shown.unit),
    enabled: checked !== "",
    staleTime: Infinity,
  });
  const duffs = checked !== "" ? parsed.data : undefined;
  const error = duffs === null ? shown.invalidAmount : duffs === 0 ? t("pay.error.invalidAmount") : null;
  return { text, setText, duffs, error, settled: checked === typed };
}

/** Pay's amount field with its fiat entry (MP-01): the form reads `fiat.error` too, so Review is
 * never open while the field shows one. */
export function usePayAmount(shown: Shown, initial: string) {
  const amount = useAmountField(shown, initial);
  return { ...amount, fiat: useFiatEntry(shown, amount) };
}

/** What the chosen coin source can spend (the engine's `maxSpendable`), for "Available"; `undefined`
 * while it loads or when "Selected coins" has none yet. Under the coin keys, so coin events refresh it,
 * except while `hold` (a sheet is up). */
export function useAvailable(walletId: string, advanced: Advanced, hold: boolean): number | undefined {
  const b = useBackend();
  const source = coinSource(advanced);
  const none = advanced.coins === "selected" && advanced.selected.length === 0;
  const available = useQuery({
    queryKey: [...keys.coinsAll(walletId), "available", source] as const,
    queryFn: () => b.maxSpendable(walletId, source, feeMode(advanced) ?? feeMode(DEFAULT_ADVANCED)!),
    enabled: !none && !hold,
    retry: false,
  });
  return none ? undefined : available.data;
}

/** The draft's live estimate of `spec` and what its refusal says: on a field, or under the form
 * (`general`, with what holds the payment back besides), or as a `notice`. `hold`: estimate
 * nothing (locked, or the form says why not). */
export function useEstimate(walletId: string, draft: DraftApi, spec: DraftSpec | null, advanced: Advanced, hold: boolean, shown: Shown, recipientRevision: string) {
  const b = useBackend();
  const { chain, amountUnit } = shown;
  // Under the wallet's coin keys: a payment or new coins refresh the estimate (events.ts).
  const estimate = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", recipientRevision, spec] as const,
    queryFn: () => draft.estimate(spec!),
    enabled: !!spec && !hold,
    retry: false,
    gcTime: 0,
  });
  // A normal payment leaves a mixing wallet's mixed and mixing coins alone: say where they are.
  const normal = spec?.source.kind === "any";
  const mixing = useQuery({ queryKey: keys.coinjoinStatus(walletId), queryFn: () => b.coinjoinStatus(walletId), retry: false, enabled: normal });
  // A disabled query keeps its cached data: the hints are for the normal source alone.
  const { fullyMixed = 0, denominated = 0, mixableUnfinished = 0 } = mixing.data?.balances ?? {};
  const stalled = denominated > fullyMixed + mixableUnfinished;
  const hints = normal ? { mixed: fullyMixed > 0, mixing: mixableUnfinished > 0, stalled } : undefined;
  const refusal = estimate.isError ? sendErrorText(estimate.error, chain, amountUnit, hints) : null;
  // A fully mixed payment refused as an absurd fee: offer Subtract fee with what it would send,
  // from the engine's own estimate of it (MX-FEE); a refusal of that too leaves the reason alone.
  const mixed = estimate.isError && spec ? mixedFeeText(estimate.error, spec, chain, amountUnit) : null;
  // With added recipients the offer would make a second one subtract the fee: none is offered.
  const subtracted = mixed && spec && spec.more.length === 0 ? { ...spec, recipient: withSubtractFee(spec.recipient) } : null;
  const offer = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", recipientRevision, subtracted] as const,
    queryFn: () => draft.estimate(subtracted!),
    enabled: !!subtracted && !hold,
    retry: false,
    gcTime: 0,
  });
  let general: string | null = null;
  let notice: PayNotice | null = null;
  if (mixed) general = mixed;
  // A refusal of a field shows under it and here too: Review's reason is the footer's.
  else if (estimate.isError) ({ general, notice } = estimateRefusal(estimate.error, !!spec && isContact(spec.recipient), refusal?.text ?? null));
  else if (advanced.coins === "selected" && advanced.selected.length === 0) general = t("pay.form.chooseCoins");
  else if (!feeMode(advanced)) general = t("pay.form.enterRate");
  else if (advanced.changeAddress !== null && advanced.changeAddress.trim() === "") general = t("pay.form.enterChangeAddress");
  // The engine names which recipient it refused (`{kind: "recipient", index}`); without, the first.
  const data = estimate.isError ? (estimate.error as { data?: { kind?: string; index?: number } | null }).data : null;
  const refusedIndex = data?.kind === "recipient" && typeof data.index === "number" ? data.index : 0;
  const fieldError = (f: "address" | "amount", index = 0) => (refusal?.field === f && refusedIndex === index ? refusal.text : null);
  const ready = !!spec && estimate.isSuccess && !estimate.isFetching && !hold;
  return { estimate, general, notice, fieldError, ready, offer: subtracted && offer.isSuccess ? offer.data.totalSent : null };
}

/** Max (U-03): everything the chosen coins can pay, the fee subtracted from it. */
export function useMax(walletId: string, advanced: Advanced, shown: Shown, set: (amountText: string) => void, othersTotal = 0, othersPending = false, recipientRevision = "") {
  const b = useBackend();
  const [error, setError] = useState<string | null>(null);
  const fee = feeMode(advanced);
  // The added recipients' total now: an answer for another total is dropped.
  const others = useRef(othersTotal);
  others.current = othersTotal;
  const revision = useRef(recipientRevision);
  revision.current = recipientRevision;
  const run = async () => {
    setError(null);
    if (!fee || othersPending) return;
    const asked = others.current;
    try {
      const max = await b.maxSpendable(walletId, coinSource(advanced), fee);
      // With added recipients, Max is what is left once they are paid.
      if (recipientRevision === revision.current && asked === others.current) set(shown.full(Math.max(0, max - asked)));
    } catch (e) {
      if (recipientRevision === revision.current) setError(sendErrorText(e, shown.chain, shown.amountUnit)?.text ?? errorMessage(e));
    }
  };
  return { run, error, disabled: !fee || othersPending };
}

/** The amount with Max, then Subtract fee (U-03), and the fiat toggle with the other unit under it
 * (MP-01). `error`: the estimate's refusal of the amount. */
export function AmountFields({
  shown,
  amount,
  max,
  error,
  available,
  coins,
  subtractFee,
  onSubtractFee,
  inputRef,
}: {
  shown: Shown;
  amount: ReturnType<typeof usePayAmount>;
  max: ReturnType<typeof useMax>;
  error: string | null;
  available: number | undefined;
  /** The coin source `available` counts. */
  coins: CoinChoice;
  subtractFee: boolean;
  onSubtractFee: (on: boolean) => void;
  inputRef?: Ref<HTMLInputElement>;
}) {
  const t = useT();
  const { fiat } = amount;
  return (
    <>
      <FormField
        label={t("pay.form.amount", { unit: fiat.unitName })}
        error={fiat.error ?? amount.error ?? error ?? max.error}
        hint={
          (available !== undefined || fiat.toggle.shown) && (
            <>
              <FiatSecondary entry={fiat} duffs={amount.duffs} testId="pay-amount-other" />
              {available !== undefined && t("pay.form.availableLine", { label: availableLabel(coins), amount: shown.full(available), unit: shown.name })}
            </>
          )
        }
      >
        {(p) => (
          <span className="amount-entry">
            <input
              {...p}
              ref={inputRef}
              data-testid="pay-amount"
              value={fiat.value}
              inputMode="decimal"
              autoComplete="off"
              placeholder={fiat.placeholder}
              onChange={(e) => fiat.onChange(e.target.value)}
            />
            <FiatToggle entry={fiat} shown={shown} testId="pay-amount-toggle" />
            <DashButton variant="tintedBlue" size="small" data-testid="pay-max" onClick={() => void max.run()} disabled={max.disabled}>
              {t("pay.form.max")}
            </DashButton>
          </span>
        )}
      </FormField>
      <label className="checkbox-row">
        <input type="checkbox" data-testid="pay-subtract-fee" checked={subtractFee} onChange={(e) => onSubtractFee(e.target.checked)} />
        {t("pay.form.subtractFee")}
      </label>
    </>
  );
}


/** The fee line, or what holds the payment back; then Review. */
export function PayFooter({
  problem,
  fee,
  shown,
  ready,
  incomplete,
  reviewRef,
  offer,
  reviewLabel,
  form = "address",
}: {
  reviewLabel?: string;
  /** Which form this is, for what an incomplete one still needs. */
  form?: PayForm;
  problem: string | null;
  /** Subtract fee would send this amount; its button turns it on (MX-FEE). Pass it only for settled fields. */
  offer?: { amount: number; accept: () => void } | null;
  /** The form has no address or amount to estimate yet (as against an estimate still being worked out). */
  incomplete: boolean;
  /** The estimate's fee, when there is one for the current form. */
  fee: number | null;
  shown: Shown;
  ready: boolean;
  reviewRef: Ref<HTMLButtonElement>;
}) {
  const t = useT();
  const waiting = incomplete ? incompleteText(form) : t("pay.checking");
  return (
    <div className="pay-footer" data-testid="pay-footer">
      <div className="pay-footer-text" aria-live="polite">
        {problem ? (
          <>
            <span className="form-problem" id="pay-reason" data-testid="pay-problem">
              {problem}
            </span>
            {/* Outside #pay-reason, so Review's description stays the reason alone. */}
            {offer && (
              <span className="form-problem" data-testid="pay-offer">
                {" "}
                {t.rich("pay.form.offer", { amount: <AmountText text={shown.full(offer.amount)} unitName={shown.name} glyph={false} /> })}{" "}
                <DashButton variant="tintedBlue" size="small" data-testid="pay-subtract-offer" onClick={offer.accept}>
                  {t("pay.form.turnOnSubtractFee")}
                </DashButton>
              </span>
            )}
          </>
        ) : fee !== null ? (
          <span className="pay-fee-line" id="pay-reason" data-testid="pay-fee-line">
            {t.rich("pay.form.networkFee", { amount: <AmountText text={shown.full(fee)} unitName={shown.name} glyph={false} /> })}
          </span>
        ) : !ready ? (
          <span className="field-hint" id="pay-reason" data-testid="pay-incomplete">
            {waiting}
          </span>
        ) : null}
      </div>
      <DashButton ref={reviewRef} type="submit" variant="filledBlue" size="large" data-testid="pay-continue" disabled={!ready} why={problem ?? waiting} whyId="pay-reason">
        {reviewLabel ?? t("pay.form.review")}
      </DashButton>
    </div>
  );
}
