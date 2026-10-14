// Pay ▸ To an address (U-03, UX-SPEC §4.7 on the mobile layout): one recipient, Max, subtract
// fee, label and message; the live fee from the engine's draft; Advanced holds coin control;
// Review opens the confirm sheet, which authenticates, signs and sends; the result follows.

import { useId, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { keys, useBackend, useBalance, useLockStatus } from "../../api/queries";
import { navigate, type PayPrefill } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { AmountText, DashButton } from "../../components/basics";
import { full, type Chain } from "../../format/amount";
import { CoinControl } from "./CoinControl";
import { ConfirmSheet } from "./ConfirmSheet";
import {
  addressProblem,
  coinSource,
  confirmedOf,
  DEFAULT_ADVANCED,
  feeMode,
  recipientOf,
  sendErrorText,
  type Advanced,
  type Confirmed,
  type Outcome,
} from "./model";
import { ResultSheet } from "./ResultSheet";
import { useDraft, type DraftSpec } from "./useDraft";

/** How long a field stays unchanged before it is checked and the draft estimated (U-03 D5). */
const SETTLE_MS = 300;

type Phase = { kind: "editing" } | { kind: "confirm"; confirmed: Confirmed; spec: DraftSpec } | { kind: "result"; outcome: Outcome };

export function chainOf(network: NetworkInfo): Chain {
  return network.kind === "mainnet" ? "main" : "test";
}

function FormField({
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

export function PayToAddress({ prefill, walletId, network }: { prefill: PayPrefill | null; walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const client = useQueryClient();
  const chain = chainOf(network);
  const locked = useLockStatus().data?.state === "locked";
  const draft = useDraft(walletId, locked);
  const balance = useBalance(walletId);

  const [address, setAddress] = useState(prefill?.address ?? "");
  const [amountText, setAmountText] = useState(prefill?.amount ? full(prefill.amount, false) : "");
  const [subtractFee, setSubtractFee] = useState(false);
  const [label, setLabel] = useState(prefill?.label ?? "");
  const [message, setMessage] = useState(prefill?.message ?? "");
  const [advanced, setAdvanced] = useState<Advanced>(DEFAULT_ADVANCED);
  const [phase, setPhase] = useState<Phase>({ kind: "editing" });
  const [maxError, setMaxError] = useState<string | null>(null);

  const addr = address.trim();
  const amt = amountText.trim();
  const checkedAddr = useDebounced(addr, SETTLE_MS);
  const checkedAmt = useDebounced(amt, SETTLE_MS);
  const settled = checkedAddr === addr && checkedAmt === amt;

  const addrClass = useQuery({
    queryKey: ["classifyAddress", network.dirName, checkedAddr],
    queryFn: () => b.classifyAddress(network.dirName, checkedAddr),
    enabled: checkedAddr !== "",
    staleTime: Infinity,
  });
  const parsed = useQuery({ queryKey: ["parseAmount", checkedAmt], queryFn: () => b.parseAmount(checkedAmt), enabled: checkedAmt !== "", staleTime: Infinity });

  const addrError = checkedAddr !== "" && addrClass.data ? addressProblem(addrClass.data) : null;
  const duffs = checkedAmt !== "" ? parsed.data : undefined;
  const amountError =
    duffs === null ? "Enter an amount in DASH, for example 0.25." : duffs === 0 ? "The amount to pay must be larger than 0." : null;

  const fee = feeMode(advanced);
  const noCoins = advanced.coins === "selected" && advanced.selected.length === 0;
  const recipient =
    addrClass.data?.kind === "core" && checkedAddr !== "" && typeof duffs === "number" && duffs > 0
      ? recipientOf({ address: checkedAddr, amount: duffs, subtractFee, label, message })
      : null;
  const spec: DraftSpec | null =
    recipient && fee && !noCoins
      ? {
          recipient,
          source: coinSource(advanced),
          fee,
          change: advanced.changeAddress ? { kind: "address", address: advanced.changeAddress } : { kind: "auto" },
        }
      : null;

  // Under the wallet's coin keys: a payment or new coins refresh the estimate (events.ts).
  const estimate = useQuery({
    queryKey: [...keys.coinsAll(walletId), "estimate", spec] as const,
    queryFn: () => draft.estimate(spec!),
    enabled: !!spec && !locked,
    retry: false,
    gcTime: 0,
  });
  const refusal = estimate.isError ? sendErrorText(estimate.error, chain) : null;
  let generalError: string | null = null;
  if (estimate.isError) generalError = refusal ? (refusal.field ? null : refusal.text) : errorMessage(estimate.error);
  else if (noCoins) generalError = "Choose the coins to pay with.";
  else if (!fee) generalError = "Enter a custom fee rate.";
  const fieldError = (f: "address" | "amount") => (refusal?.field === f ? refusal.text : null);
  const ready = !!spec && settled && estimate.isSuccess && !estimate.isFetching && !locked;

  const useMax = async () => {
    setMaxError(null);
    if (!fee) return;
    try {
      const max = await b.maxSpendable(walletId, coinSource(advanced), fee);
      setAmountText(full(max, false));
      setSubtractFee(true);
    } catch (e) {
      setMaxError(sendErrorText(e, chain)?.text ?? errorMessage(e));
    }
  };

  const review = () => {
    if (ready && estimate.data && spec) setPhase({ kind: "confirm", confirmed: confirmedOf(spec.recipient, estimate.data), spec });
  };

  /** A payment left the editing form behind: start a new one (the draft too). */
  const reset = () => {
    setAddress("");
    setAmountText("");
    setSubtractFee(false);
    setLabel("");
    setMessage("");
    setAdvanced(DEFAULT_ADVANCED);
    void draft.close();
    void client.invalidateQueries({ queryKey: keys.coinsAll(walletId) });
  };

  const finish = (outcome: Outcome, show: boolean) => {
    // A sent payment, and one that may have been, ends the form: paying again is a new payment.
    if (outcome.kind !== "failed") reset();
    setPhase({ kind: "editing" });
    if (show && outcome.kind === "sent") navigate("activity", { detail: outcome.txid });
  };

  const available = balance.data?.confirmed;
  return (
    <>
      <form
        className="pay-form"
        data-testid="pay-address-form"
        aria-label="Pay to an address"
        onSubmit={(e) => {
          e.preventDefault();
          review();
        }}
      >
        {prefill?.username && (
          <p className="placeholder-note" data-testid="pay-username">
            The link names the payee @{prefill.username}. The name is not verified.
          </p>
        )}
        <section className="pay-card card-l1">
          <FormField label="Pay to" error={addrError ?? fieldError("address")}>
            {(p) => (
              <input
                {...p}
                data-testid="pay-address"
                value={address}
                autoComplete="off"
                spellCheck={false}
                placeholder="Dash address"
                onChange={(e) => setAddress(e.target.value)}
              />
            )}
          </FormField>
          <FormField
            label={`Amount (${network.unitName})`}
            error={amountError ?? fieldError("amount") ?? maxError}
            hint={available !== undefined && <>Available: {full(available, false)} {network.unitName}</>}
          >
            {(p) => (
              <span className="amount-entry">
                <input
                  {...p}
                  data-testid="pay-amount"
                  value={amountText}
                  inputMode="decimal"
                  autoComplete="off"
                  placeholder="0.00"
                  onChange={(e) => setAmountText(e.target.value)}
                />
                <DashButton variant="tintedBlue" size="small" data-testid="pay-max" onClick={() => void useMax()} disabled={!fee}>
                  Max
                </DashButton>
              </span>
            )}
          </FormField>
          <label className="checkbox-row">
            <input type="checkbox" data-testid="pay-subtract-fee" checked={subtractFee} onChange={(e) => setSubtractFee(e.target.checked)} />
            Subtract fee from amount
          </label>
          <FormField label="Label" hint="Saved to your address book with the address.">
            {(p) => <input {...p} data-testid="pay-label" value={label} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />}
          </FormField>
          <FormField label="Message" hint="Kept with the transaction in this wallet; not sent to the payee.">
            {(p) => <input {...p} data-testid="pay-message" value={message} autoComplete="off" onChange={(e) => setMessage(e.target.value)} />}
          </FormField>
        </section>
        <CoinControl walletId={walletId} network={network} advanced={advanced} onChange={setAdvanced} payAmount={typeof duffs === "number" ? duffs : null} />
        <div className="pay-footer" data-testid="pay-footer">
          <div className="pay-footer-text" aria-live="polite">
            {generalError ? (
              <span className="form-problem" data-testid="pay-problem">
                {generalError}
              </span>
            ) : estimate.data && spec ? (
              <span className="pay-fee-line" data-testid="pay-fee-line">
                Network fee <AmountText text={full(estimate.data.fee, false)} unitName={network.unitName} glyph={false} />
              </span>
            ) : null}
          </div>
          <DashButton type="submit" variant="filledBlue" size="large" data-testid="pay-continue" disabled={!ready}>
            Review
          </DashButton>
        </div>
      </form>
      {phase.kind === "confirm" && (
        <ConfirmSheet
          confirmed={phase.confirmed}
          spec={phase.spec}
          coins={advanced}
          walletId={walletId}
          network={network}
          draft={draft}
          onCancel={() => setPhase({ kind: "editing" })}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
        />
      )}
      {phase.kind === "result" && (
        <ResultSheet
          outcome={phase.outcome}
          network={network}
          draft={draft}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
          onDone={(show) => finish(phase.outcome, show)}
        />
      )}
    </>
  );
}
