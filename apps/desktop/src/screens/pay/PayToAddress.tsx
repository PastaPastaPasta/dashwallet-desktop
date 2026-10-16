// Pay ▸ To an address (U-03, UX-SPEC §4.7 on the mobile layout): one recipient, Max, subtract
// fee, label and message; the live fee from the engine's draft; Advanced holds coin control;
// Review opens the confirm sheet, which authenticates, signs and sends; the result follows.

import { useCallback, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { keys, useBackend, useBalance, useLockStatus } from "../../api/queries";
import { navigate, ui, type PayPrefill } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton } from "../../components/basics";
import { AddressBookPicker } from "../tools/AddressBook";
import { useDisplay } from "../../format/display";
import { CoinControl } from "./CoinControl";
import { ConfirmSheet } from "./ConfirmSheet";
import { draftSpec, FormField, PayFooter, SETTLE_MS, useAmountField, useEstimate, useMax } from "./form";
import { addressProblem, confirmedOf, DEFAULT_ADVANCED, recipientOf, type Advanced, type Confirmed, type Outcome } from "./model";
import { ResultSheet } from "./ResultSheet";
import { useDraft, type DraftSpec } from "./useDraft";

export type Phase = { kind: "editing" } | { kind: "confirm"; confirmed: Confirmed; spec: DraftSpec } | { kind: "result"; outcome: Outcome };

export function PayToAddress({
  prefill,
  coins,
  walletId,
  network,
}: {
  prefill: PayPrefill | null;
  /** Coin Selection's coins (U-05 D19): Advanced starts on "Selected coins" with them. */
  coins: string[] | null;
  walletId: string;
  network: NetworkInfo;
}) {
  const b = useBackend();
  const client = useQueryClient();
  const shown = useDisplay(network);
  const locked = useLockStatus().data?.state === "locked";
  const draft = useDraft(walletId, locked);
  const balance = useBalance(walletId);

  const [address, setAddress] = useState(prefill?.address ?? "");
  const amount = useAmountField(shown, prefill?.amount ? shown.full(prefill.amount) : "");
  // Settings ▸ General's default starts each new form (U-04 D5); Max still turns it on.
  const subtractFeeByDefault = () => ui.get().prefs.subtractFeeByDefault;
  const [subtractFee, setSubtractFee] = useState(subtractFeeByDefault);
  const [label, setLabel] = useState(prefill?.label ?? "");
  const [message, setMessage] = useState(prefill?.message ?? "");
  const [advanced, setAdvanced] = useState<Advanced>(coins?.length ? { ...DEFAULT_ADVANCED, coins: "selected", selected: coins } : DEFAULT_ADVANCED);
  const [phase, setPhase] = useState<Phase>({ kind: "editing" });
  // Closing the confirm or result sheet returns to Review, or to the address of a cleared form.
  const reviewButton = useRef<HTMLButtonElement>(null);
  const addressField = useRef<HTMLInputElement>(null);
  const backToForm = useCallback(() => (reviewButton.current?.disabled === false ? reviewButton.current : addressField.current), []);
  // The sending address book (QT-097): choosing fills the address, and its label if none was typed.
  const [picking, setPicking] = useState(false);
  const chosen = useCallback((a: string, l: string) => {
    setAddress(a);
    setLabel((old) => old || l);
    setPicking(false);
    addressField.current?.focus();
  }, []);

  const addr = address.trim();
  const checkedAddr = useDebounced(addr, SETTLE_MS);
  const settled = checkedAddr === addr && amount.settled;

  const addrClass = useQuery({
    queryKey: ["classifyAddress", network.dirName, checkedAddr],
    queryFn: () => b.classifyAddress(network.dirName, checkedAddr),
    enabled: checkedAddr !== "",
    staleTime: Infinity,
  });

  const addrError = checkedAddr !== "" && addrClass.data ? addressProblem(addrClass.data) : null;
  const { duffs } = amount;

  const recipient =
    addrClass.data?.kind === "core" && checkedAddr !== "" && typeof duffs === "number" && duffs > 0
      ? recipientOf({ address: checkedAddr, amount: duffs, subtractFee, label, message })
      : null;
  const spec = draftSpec(recipient, advanced);

  const { estimate, general: generalError, fieldError, ready: estimated } = useEstimate(walletId, draft, spec, advanced, locked, shown);
  const ready = estimated && settled;
  const max = useMax(walletId, advanced, shown, (text) => {
    amount.setText(text);
    setSubtractFee(true);
  });

  const review = () => {
    if (ready && estimate.data && spec) setPhase({ kind: "confirm", confirmed: confirmedOf(spec.recipient, estimate.data, spec.change), spec });
  };

  /** A payment left the editing form behind: start a new one (the draft too). */
  const reset = () => {
    setAddress("");
    amount.setText("");
    setSubtractFee(subtractFeeByDefault());
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
              <span className="amount-entry">
                <input
                  {...p}
                  ref={addressField}
                  data-testid="pay-address"
                  value={address}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="Dash address"
                  onChange={(e) => setAddress(e.target.value)}
                />
                <DashButton variant="tintedBlue" size="small" data-testid="pay-address-book" onClick={() => setPicking(true)}>
                  Address Book…
                </DashButton>
              </span>
            )}
          </FormField>
          <FormField
            label={`Amount (${shown.name})`}
            error={amount.error ?? fieldError("amount") ?? max.error}
            hint={available !== undefined && <>Available: {shown.full(available)} {shown.name}</>}
          >
            {(p) => (
              <span className="amount-entry">
                <input
                  {...p}
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
        <PayFooter problem={generalError} fee={estimate.data && spec ? estimate.data.fee : null} shown={shown} ready={ready} reviewRef={reviewButton} />
      </form>
      {picking && <AddressBookPicker walletId={walletId} network={network} purpose="send" onChoose={chosen} onClose={() => setPicking(false)} />}
      {phase.kind === "confirm" && (
        <ConfirmSheet
          confirmed={phase.confirmed}
          spec={phase.spec}
          coins={advanced}
          walletId={walletId}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          onCancel={() => setPhase({ kind: "editing" })}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
        />
      )}
      {phase.kind === "result" && (
        <ResultSheet
          outcome={phase.outcome}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          onOutcome={(outcome) => setPhase({ kind: "result", outcome })}
          onDone={(show) => finish(phase.outcome, show)}
        />
      )}
    </>
  );
}
