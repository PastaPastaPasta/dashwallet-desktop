// What Pay's two forms share (U-03; DP3-06 D8): a labelled field, the amount as typed and as the
// engine parses it, the live estimate of the draft and the refusal it brings, Max, and the footer
// with the fee line and Review.

import { useId, useState, type ReactNode, type Ref } from "react";
import { useQuery } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import { keys, useBackend } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { AmountText, DashButton } from "../../components/basics";
import type { useDisplay } from "../../format/display";
import { availableLabel, changePolicy, coinSource, DEFAULT_ADVANCED, feeMode, mixedFeeText, sendErrorText, withSubtractFee, type Advanced, type CoinChoice, type Confirmed, type Outcome, type PayRecipient } from "./model";
import type { DraftApi, DraftSpec } from "./useDraft";

/** How long a field stays unchanged before it is checked and the draft estimated (U-03 D5). */
export const SETTLE_MS = 300;

type Shown = ReturnType<typeof useDisplay>;

/** Where a form is: editing, confirming, or showing the result. */
export type Phase = { kind: "editing" } | { kind: "confirm"; confirmed: Confirmed; spec: DraftSpec } | { kind: "result"; outcome: Outcome };

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

/** The draft paying `recipient` with the Advanced choices; `null` while either is incomplete. */
export function draftSpec(recipient: PayRecipient | null, a: Advanced): DraftSpec | null {
  const fee = feeMode(a);
  const change = changePolicy(a);
  const noCoins = a.coins === "selected" && a.selected.length === 0;
  return recipient && fee && change && !noCoins ? { recipient, source: coinSource(a), fee, change } : null;
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
  const error = duffs === null ? shown.invalidAmount : duffs === 0 ? "The amount to pay must be larger than 0." : null;
  return { text, setText, duffs, error, settled: checked === typed };
}

/** What the chosen coin source can spend (the engine's `maxSpendable`), for "Available"; `undefined`
 * while it loads or when "Selected coins" has none yet. Under the coin keys, so coin events refresh it. */
export function useAvailable(walletId: string, advanced: Advanced): number | undefined {
  const b = useBackend();
  const source = coinSource(advanced);
  const none = advanced.coins === "selected" && advanced.selected.length === 0;
  const available = useQuery({
    queryKey: [...keys.coinsAll(walletId), "available", source] as const,
    queryFn: () => b.maxSpendable(walletId, source, feeMode(advanced) ?? feeMode(DEFAULT_ADVANCED)!),
    enabled: !none,
    retry: false,
  });
  return none ? undefined : available.data;
}

/** The draft's live estimate of `spec` and what its refusal says: on a field, or under the form
 * (`general`, with what holds the payment back besides). `hold`: estimate nothing (locked, or the
 * form says why not). */
export function useEstimate(walletId: string, draft: DraftApi, spec: DraftSpec | null, advanced: Advanced, hold: boolean, shown: Shown) {
  const b = useBackend();
  const { chain, amountUnit } = shown;
  // Under the wallet's coin keys: a payment or new coins refresh the estimate (events.ts).
  const estimate = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", spec] as const,
    queryFn: () => draft.estimate(spec!),
    enabled: !!spec && !hold,
    retry: false,
    gcTime: 0,
  });
  // A normal payment leaves a mixing wallet's mixed and mixing coins alone: say where they are.
  const normal = spec?.source.kind === "any";
  const mixing = useQuery({ queryKey: keys.coinjoinStatus(walletId), queryFn: () => b.coinjoinStatus(walletId), retry: false, enabled: normal });
  // A disabled query keeps its cached data: the hints are for the normal source alone.
  const { fullyMixed = 0, denominated = 0 } = mixing.data?.balances ?? {};
  const hints = normal ? { mixed: fullyMixed > 0, mixing: denominated > fullyMixed } : undefined;
  const refusal = estimate.isError ? sendErrorText(estimate.error, chain, amountUnit, hints) : null;
  // A fully mixed payment refused as an absurd fee: offer Subtract fee with what it would send,
  // from the engine's own estimate of it (MX-FEE); a refusal of that too leaves the reason alone.
  const mixed = estimate.isError && spec ? mixedFeeText(estimate.error, spec, chain, amountUnit) : null;
  const subtracted = mixed && spec ? { ...spec, recipient: withSubtractFee(spec.recipient) } : null;
  const offer = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", subtracted] as const,
    queryFn: () => draft.estimate(subtracted!),
    enabled: !!subtracted && !hold,
    retry: false,
    gcTime: 0,
  });
  let general: string | null = null;
  if (mixed) general = mixed;
  // A refusal of a field shows under it and here too: Review's reason is the footer's.
  else if (estimate.isError) general = refusal ? refusal.text : errorMessage(estimate.error);
  else if (advanced.coins === "selected" && advanced.selected.length === 0) general = "Choose the coins to pay with.";
  else if (!feeMode(advanced)) general = "Enter a custom fee rate.";
  else if (advanced.changeAddress !== null && advanced.changeAddress.trim() === "") general = "Enter the custom change address.";
  const fieldError = (f: "address" | "amount") => (refusal?.field === f ? refusal.text : null);
  const ready = !!spec && estimate.isSuccess && !estimate.isFetching && !hold;
  return { estimate, general, fieldError, ready, offer: subtracted && offer.isSuccess ? offer.data.totalSent : null };
}

/** Max (U-03): everything the chosen coins can pay, the fee subtracted from it. */
export function useMax(walletId: string, advanced: Advanced, shown: Shown, set: (amountText: string) => void) {
  const b = useBackend();
  const [error, setError] = useState<string | null>(null);
  const fee = feeMode(advanced);
  const run = async () => {
    setError(null);
    if (!fee) return;
    try {
      set(shown.full(await b.maxSpendable(walletId, coinSource(advanced), fee)));
    } catch (e) {
      setError(sendErrorText(e, shown.chain, shown.amountUnit)?.text ?? errorMessage(e));
    }
  };
  return { run, error, disabled: !fee };
}

/** The amount with Max, then Subtract fee (U-03). `error`: the estimate's refusal of the amount. */
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
  amount: ReturnType<typeof useAmountField>;
  max: ReturnType<typeof useMax>;
  error: string | null;
  available: number | undefined;
  /** The coin source `available` counts. */
  coins: CoinChoice;
  subtractFee: boolean;
  onSubtractFee: (on: boolean) => void;
  inputRef?: Ref<HTMLInputElement>;
}) {
  return (
    <>
      <FormField
        label={`Amount (${shown.name})`}
        error={amount.error ?? error ?? max.error}
        hint={available !== undefined && <>{availableLabel(coins)}: {shown.full(available)} {shown.name}</>}
      >
        {(p) => (
          <span className="amount-entry">
            <input
              {...p}
              ref={inputRef}
              data-testid="pay-amount"
              value={amount.text}
              inputMode="decimal"
              autoComplete="off"
              placeholder={shown.placeholder}
              onChange={(e) => amount.setText(e.target.value)}
            />
            <DashButton variant="tintedBlue" size="small" data-testid="pay-max" onClick={() => void max.run()} disabled={max.disabled}>
              Max
            </DashButton>
          </span>
        )}
      </FormField>
      <label className="checkbox-row">
        <input type="checkbox" data-testid="pay-subtract-fee" checked={subtractFee} onChange={(e) => onSubtractFee(e.target.checked)} />
        Subtract fee from amount
      </label>
    </>
  );
}

const PAY_INCOMPLETE = "Enter an address and an amount first.";
const PAY_CHECKING = "Checking the payment…";

/** The fee line, or what holds the payment back; then Review. */
export function PayFooter({
  problem,
  fee,
  shown,
  ready,
  incomplete,
  reviewRef,
  offer,
}: {
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
  const waiting = incomplete ? PAY_INCOMPLETE : PAY_CHECKING;
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
                {" "}With Subtract fee it would send <AmountText text={shown.full(offer.amount)} unitName={shown.name} glyph={false} />.{" "}
                <DashButton variant="tintedBlue" size="small" data-testid="pay-subtract-offer" onClick={offer.accept}>
                  Turn on Subtract fee
                </DashButton>
              </span>
            )}
          </>
        ) : fee !== null ? (
          <span className="pay-fee-line" id="pay-reason" data-testid="pay-fee-line">
            Network fee <AmountText text={shown.full(fee)} unitName={shown.name} glyph={false} />
          </span>
        ) : !ready ? (
          <span className="field-hint" id="pay-reason" data-testid="pay-incomplete">
            {waiting}
          </span>
        ) : null}
      </div>
      <DashButton ref={reviewRef} type="submit" variant="filledBlue" size="large" data-testid="pay-continue" disabled={!ready} why={problem ?? waiting} whyId="pay-reason">
        Review
      </DashButton>
    </div>
  );
}
