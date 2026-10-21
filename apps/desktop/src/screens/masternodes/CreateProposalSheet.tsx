// Create a proposal (§4.25.3, D14, MG-06): Details, Payments, then Review and pay the fee. Review
// reads `preview_proposal`; [Pay 1 Dash and create] asks a spend grant capped at the fee plus the
// network fee, then `create_proposal`. An engine field refusal goes back to the first failing
// field's step. Once paid, the proposal is the engine's (Mine shows it and resumes it after a
// restart, D27).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState } from "react";
import { isAppError } from "../../api/backend";
import type { MyProposal } from "../../api/generated/MyProposal";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalDraft } from "../../api/generated/ProposalDraft";
import type { ProposalFieldName } from "../../api/generated/ProposalFieldName";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import {
  budgetNotice,
  defaultFirstPayment,
  detailsProblems,
  FIELD_STEP,
  fieldText,
  GovText,
  governanceErrorText,
  lastPaymentText,
  MAX_PAYMENTS,
  shortAddress,
  shortDate,
  superblockOption,
} from "../../governance/model";
import { useGovernanceInfo, useGovernanceRefresh, useUpcomingSuperblocks } from "../../governance/queries";
import { useAmountField } from "../pay/form";
import { failureText } from "../pay/model";
import { useNowSecs } from "./common";
import { CredentialField, useCredential } from "./credential";

type Step = 1 | 2 | 3;
const STEPS: { n: Step; title: string; heading: string }[] = [
  { n: 1, title: "Proposal", heading: "What is the proposal?" },
  { n: 2, title: "Payments", heading: "How should it be paid?" },
  { n: 3, title: "Review and pay the fee", heading: "Review the proposal" },
];

type FieldErrors = Partial<Record<ProposalFieldName, string>>;

export function CreateProposalSheet({
  wallet,
  network,
  onClose,
  onCreated,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  onClose: () => void;
  onCreated: (p: MyProposal) => void;
}) {
  const b = useBackend();
  const shown = useDisplay(network);
  const now = useNowSecs();
  const info = useGovernanceInfo();
  const refresh = useGovernanceRefresh();
  const superblocks = useUpcomingSuperblocks();
  const id = useId();
  const [step, setStep] = useState<Step>(1);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [first, setFirst] = useState<number | null>(null);
  const [count, setCount] = useState(1);
  const amount = useAmountField(shown, "");
  const [payTo, setPayTo] = useState<"new" | "other">("new");
  const [other, setOther] = useState("");
  /** The new receive address, taken once per sheet (labelled with the proposal's name). */
  const [fresh, setFresh] = useState<string | null>(null);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [general, setGeneral] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const credential = useCredential(wallet.walletId, "Enter your passphrase to pay the fee.");

  const choice = useMemo(() => defaultFirstPayment(superblocks.data ?? [], now), [superblocks.data, now]);
  useEffect(() => {
    if (first === null && choice.height !== null) setFirst(choice.height);
  }, [first, choice.height]);

  const address = payTo === "new" ? fresh : other.trim();
  const draft: ProposalDraft | null =
    step === 3 && address && first !== null && amount.duffs
      ? { name, url: url.trim(), paymentAddress: address, paymentAmount: amount.duffs, paymentCount: count, firstSuperblockHeight: first, coins: [] }
      : null;
  const preview = useQuery({
    queryKey: ["governance", "preview", wallet.walletId, draft] as const,
    queryFn: () => b.governancePreviewProposal(wallet.walletId, draft!),
    enabled: draft !== null,
    retry: false,
    gcTime: 0,
  });

  /** An engine refusal of the draft's fields: back to the first failing field's step. */
  const refuseFields = (e: unknown): boolean => {
    if (!isAppError(e) || e.code !== "governance.invalid_proposal" || e.data?.kind !== "proposalFields") return false;
    const fields = e.data.fields;
    setErrors(Object.fromEntries(fields.map((f) => [f, fieldText(f, network.kind)])));
    setStep(fields.reduce<Step>((s, f) => Math.min(s, FIELD_STEP[f]) as Step, 3));
    return true;
  };
  useEffect(() => {
    if (preview.isError && !refuseFields(preview.error)) setGeneral(governanceErrorText(preview.error) ?? failureText(preview.error, shown.chain, shown.amountUnit));
    // Read once per failed preview.
  }, [preview.isError, preview.error]);

  const next = async () => {
    setGeneral(null);
    if (step === 1) {
      const p = detailsProblems(name, url);
      setErrors(p);
      if (Object.keys(p).length === 0) setStep(2);
      return;
    }
    if (step === 2) {
      const p: FieldErrors = {};
      if (first === null) p.first_payment = fieldText("first_payment", network.kind);
      if (amount.error || !amount.duffs) p.payment_amount = amount.error ?? "Enter the amount of each payment.";
      if (payTo === "other" && other.trim() === "") p.payment_address = fieldText("payment_address", network.kind);
      setErrors(p);
      if (Object.keys(p).length > 0 || !amount.settled) return;
      if (payTo === "new" && fresh === null) {
        try {
          setFresh((await b.nextReceiveAddress(wallet.walletId, `Proposal ${name}`)).address);
        } catch (e) {
          setGeneral(failureText(e, shown.chain, shown.amountUnit));
          return;
        }
      }
      setStep(3);
    }
  };

  const create = async () => {
    const p = preview.data;
    if (busy || !draft || !p || !credential.ready) return;
    setBusy(true);
    setGeneral(null);
    try {
      const grant = await credential.authorize({ kind: "spend", maxDuffs: p.fee + p.networkFee });
      if (!grant) return;
      const created = await b.governanceCreateProposal(wallet.walletId, draft, grant.id);
      void refresh();
      onCreated(created);
    } catch (e) {
      if (!refuseFields(e)) credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
    } finally {
      setBusy(false);
    }
  };

  const fieldProps = (f: ProposalFieldName) => ({
    "aria-invalid": errors[f] ? true : undefined,
    "aria-describedby": errors[f] ? `${id}-${f}-error` : undefined,
  });
  const fieldError = (f: ProposalFieldName) =>
    errors[f] && (
      <span className="mg-field-error" id={`${id}-${f}-error`} data-testid={`create-error-${f}`}>
        {errors[f]}
      </span>
    );
  const sb = superblocks.data ?? [];
  const firstSb = sb.find((s) => s.height === first);
  const lastSb = sb[sb.findIndex((s) => s.height === first) + count - 1];
  const total = amount.duffs ? amount.duffs * count : null;
  const notice = budgetNotice(info.data, (d) => `${shown.compact(d)} ${shown.name}`);

  let body;
  if (step === 1)
    body = (
      <div className="mg-form">
        <label className={`mg-field mg-span ${errors.name ? "has-error" : ""}`}>
          Name
          <input data-testid="create-name" className="mono" value={name} maxLength={40} autoComplete="off" data-autofocus onChange={(e) => setName(e.target.value)} {...fieldProps("name")} />
          <span className="mg-field-help">{GovText.nameHelp}</span>
          {fieldError("name")}
        </label>
        <label className={`mg-field mg-span ${errors.url ? "has-error" : ""}`}>
          Link
          <input data-testid="create-url" value={url} placeholder="https://" onChange={(e) => setUrl(e.target.value)} {...fieldProps("url")} />
          <span className="mg-field-help">{GovText.linkHelp}</span>
          {fieldError("url")}
          {fieldError("payload")}
        </label>
      </div>
    );
  else if (step === 2)
    body = (
      <>
        {superblocks.isPending ? (
          <LoadingState testId="create-superblocks" text="Reading the next superblocks…" />
        ) : (
          <div className="mg-form">
            <label className="mg-field">
              First payment
              <select data-testid="create-first" value={first ?? ""} onChange={(e) => setFirst(Number(e.target.value))} {...fieldProps("first_payment")}>
                {sb.map((s) => (
                  <option key={s.height} value={s.height}>
                    {superblockOption(s)}
                  </option>
                ))}
              </select>
              <span className="mg-field-help">{GovText.firstPaymentHelp}</span>
              {fieldError("first_payment")}
            </label>
            <label className="mg-field">
              Number of payments
              <select data-testid="create-count" value={count} onChange={(e) => setCount(Number(e.target.value))} {...fieldProps("payment_count")}>
                {Array.from({ length: MAX_PAYMENTS }, (_, i) => i + 1).map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
              <span className="mg-field-help">{lastPaymentText(sb, first, count)}</span>
              {fieldError("payment_count")}
            </label>
            <label className={`mg-field ${errors.payment_amount ? "has-error" : ""}`}>
              Amount per payment
              <span className="mg-field-row">
                <input
                  data-testid="create-amount"
                  inputMode="decimal"
                  placeholder={shown.placeholder}
                  value={amount.text}
                  onChange={(e) => amount.setText(e.target.value)}
                  {...fieldProps("payment_amount")}
                />
                <span>{shown.name}</span>
              </span>
              {fieldError("payment_amount")}
            </label>
            <div className="mg-field">
              Total
              <span className="mg-field-row mg-total" data-testid="create-total">
                {total !== null ? shown.withUnit(total) : "—"}
              </span>
            </div>
            <label className={`mg-field mg-span ${errors.payment_address ? "has-error" : ""}`}>
              Pay to
              <select data-testid="create-pay-to" value={payTo} onChange={(e) => setPayTo(e.target.value as "new" | "other")}>
                <option value="new">A new address in this wallet</option>
                <option value="other">Another address…</option>
              </select>
              {payTo === "other" && (
                <input
                  data-testid="create-address"
                  className="mono"
                  aria-label="Address to pay"
                  value={other}
                  autoComplete="off"
                  onChange={(e) => setOther(e.target.value)}
                  {...fieldProps("payment_address")}
                />
              )}
              {fieldError("payment_address")}
            </label>
          </div>
        )}
        {choice.notice && first === choice.height && (
          <p className="mg-notice mg-notice-warning" data-testid="create-cutoff-notice">
            {choice.notice}
          </p>
        )}
        {notice && (
          <p className="mg-notice mg-notice-info" data-testid="create-budget-notice">
            {notice}
          </p>
        )}
      </>
    );
  else
    body = (
      <>
        {preview.isPending && draft ? (
          <LoadingState testId="create-preview-loading" text="Checking the proposal…" />
        ) : preview.data && draft ? (
          <>
            <dl className="detail-list card-l1" data-testid="create-review">
              <DetailRow label="Name">
                <span className="mono">{draft.name}</span>
              </DetailRow>
              <DetailRow label="Link">{draft.url}</DetailRow>
              <DetailRow label="Payments" testId="create-review-payments">
                <AmountText text={shown.compact(draft.paymentAmount)} unitName={shown.name} /> × {draft.paymentCount}
                {firstSb && `, ${shortDate(firstSb.estimatedTime)}`}
                {lastSb && draft.paymentCount > 1 && ` to ${shortDate(lastSb.estimatedTime)}`} · total{" "}
                <AmountText text={shown.compact(draft.paymentAmount * draft.paymentCount)} unitName={shown.name} />
              </DetailRow>
              <DetailRow label="Paid to">
                <span className="mono" title={draft.paymentAddress}>
                  {shortAddress(draft.paymentAddress)}
                </span>
                {payTo === "new" && ", this wallet"}
              </DetailRow>
              <DetailRow label="Fee" testId="create-review-fee">
                <AmountText text={shown.full(preview.data.fee)} unitName={shown.name} /> + network fee{" "}
                <AmountText text={shown.full(preview.data.networkFee)} unitName={shown.name} />
              </DetailRow>
            </dl>
            <details className="disclosure card-l1">
              <summary>Proposal data (JSON)</summary>
              <pre className="mono mg-json" data-testid="create-json">
                {preview.data.dataJson}
              </pre>
            </details>
            <p className="mg-notice mg-notice-warning" data-testid="create-fee-warning">
              <strong>{GovText.feeWarning}</strong> {GovText.feeAfter}
            </p>
            <CredentialField c={credential} busy={busy} testId="create" className="mg-create-credential" />
          </>
        ) : null}
      </>
    );

  const current = STEPS.find((s) => s.n === step)!;
  return (
    <Sheet title="Create a proposal" width="large" testId="create-sheet" closable={!busy} focusKey={String(step)} onClose={onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void (step === 3 ? create() : next());
        }}
      >
        <div className="mg-wizard">
          <ol className="mg-steps" aria-label="Steps">
            {STEPS.map((s) => (
              <li key={s.n} data-n={s.n} className={s.n < step ? "is-done" : undefined} aria-current={s.n === step ? "step" : undefined}>
                {s.title}
              </li>
            ))}
          </ol>
          <div className="mg-step" data-testid={`create-step-${step}`}>
            <h3 className="mg-step-title">{current.heading}</h3>
            {body}
            <p className="form-problem" role="alert" data-testid="create-general">
              {general}
            </p>
          </div>
        </div>
        <footer className="mg-sheet-footer">
          <span className="mg-sheet-footer-note">
            Step {current.n} of {STEPS.length}
          </span>
          <span className="mg-actions">
            {step > 1 ? (
              <DashButton variant="tintedGray" size="medium" data-testid="create-back" disabled={busy} onClick={() => setStep((s) => (s - 1) as Step)}>
                Back
              </DashButton>
            ) : (
              <DashButton variant="tintedGray" size="medium" data-testid="create-cancel" onClick={onClose}>
                Cancel
              </DashButton>
            )}
            {step < 3 ? (
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="create-continue" disabled={step === 2 && !amount.settled}>
                Continue
              </DashButton>
            ) : (
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="create-pay" disabled={busy || !preview.data || !credential.ready}>
                {busy ? "Paying…" : `Pay ${shown.compact(preview.data?.fee ?? 100_000_000)} ${shown.name} and create`}
              </DashButton>
            )}
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
