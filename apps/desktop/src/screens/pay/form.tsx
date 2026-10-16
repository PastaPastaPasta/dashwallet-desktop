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
import { changePolicy, coinSource, feeMode, sendErrorText, type Advanced, type PayRecipient } from "./model";
import type { DraftApi, DraftSpec } from "./useDraft";

/** How long a field stays unchanged before it is checked and the draft estimated (U-03 D5). */
export const SETTLE_MS = 300;

type Shown = ReturnType<typeof useDisplay>;

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

/** The draft's live estimate of `spec` and what its refusal says: on a field, or under the form
 * (`general`, with what holds the payment back besides). `hold`: estimate nothing (locked, or the
 * form says why not). */
export function useEstimate(walletId: string, draft: DraftApi, spec: DraftSpec | null, advanced: Advanced, hold: boolean, shown: Shown) {
  const { chain, amountUnit } = shown;
  // Under the wallet's coin keys: a payment or new coins refresh the estimate (events.ts).
  const estimate = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", spec] as const,
    queryFn: () => draft.estimate(spec!),
    enabled: !!spec && !hold,
    retry: false,
    gcTime: 0,
  });
  const refusal = estimate.isError ? sendErrorText(estimate.error, chain, amountUnit) : null;
  let general: string | null = null;
  if (estimate.isError) general = refusal ? (refusal.field ? null : refusal.text) : errorMessage(estimate.error);
  else if (advanced.coins === "selected" && advanced.selected.length === 0) general = "Choose the coins to pay with.";
  else if (!feeMode(advanced)) general = "Enter a custom fee rate.";
  else if (advanced.changeAddress !== null && advanced.changeAddress.trim() === "") general = "Enter the custom change address.";
  const fieldError = (f: "address" | "amount") => (refusal?.field === f ? refusal.text : null);
  const ready = !!spec && estimate.isSuccess && !estimate.isFetching && !hold;
  return { estimate, general, fieldError, ready };
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

/** The fee line, or what holds the payment back; then Review. */
export function PayFooter({
  problem,
  fee,
  shown,
  ready,
  reviewRef,
}: {
  problem: string | null;
  /** The estimate's fee, when there is one for the current form. */
  fee: number | null;
  shown: Shown;
  ready: boolean;
  reviewRef: Ref<HTMLButtonElement>;
}) {
  return (
    <div className="pay-footer" data-testid="pay-footer">
      <div className="pay-footer-text" aria-live="polite">
        {problem ? (
          <span className="form-problem" data-testid="pay-problem">
            {problem}
          </span>
        ) : fee !== null ? (
          <span className="pay-fee-line" data-testid="pay-fee-line">
            Network fee <AmountText text={shown.full(fee)} unitName={shown.name} glyph={false} />
          </span>
        ) : null}
      </div>
      <DashButton ref={reviewRef} type="submit" variant="filledBlue" size="large" data-testid="pay-continue" disabled={!ready}>
        Review
      </DashButton>
    </div>
  );
}
