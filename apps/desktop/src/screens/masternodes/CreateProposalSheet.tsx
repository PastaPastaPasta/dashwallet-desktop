// Create a proposal (§4.25.3, D14, MG-06): Details, Payments, then Review and pay the fee. Review
// reads `preview_proposal`; [Pay 1 Dash and create] reads it again (the coins may have moved): a
// changed fee stops for a fresh confirm (D33), the same one asks a spend grant capped at the
// confirmed total plus dwcli's margin (D29), then `create_proposal`. An engine field refusal goes
// back to the first failing field's step. Any other failure may come after the fee went out, so
// Pay waits for a re-read of Mine and, if the proposal is there, sends the user to it (D34). Once
// paid, the proposal is the engine's (Mine shows it and resumes it after a restart, D27).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { isAppError } from "../../api/backend";
import type { MyProposal } from "../../api/generated/MyProposal";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProposalDraft } from "../../api/generated/ProposalDraft";
import type { ProposalFieldName } from "../../api/generated/ProposalFieldName";
import type { ProposalPreview } from "../../api/generated/ProposalPreview";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import {
  budgetNotice,
  createdFrom,
  defaultFirstPayment,
  detailsProblems,
  feeGrantCap,
  feeTotal,
  FIELD_STEP,
  fieldText,
  GovText,
  governanceErrorText,
  lastPaymentText,
  MAX_PAYMENTS,
  sameFee,
  shortAddress,
  shortDate,
  superblockOption,
} from "../../governance/model";
import { useGovernanceInfo, useGovernanceRefresh, useMyProposals, useUpcomingSuperblocks } from "../../governance/queries";
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

/** After a failed `create_proposal` (D34): reading Mine, the proposal it made anyway, or Mine
 *  unreadable. Pay stays off in all three. */
type AfterFailure = { kind: "checking" } | { kind: "created"; p: MyProposal } | { kind: "unknown"; known: ReadonlySet<string> };

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
  /** The preview the review shows, for the draft it was read for: the first one, then only Pay's
   *  own re-read moves it, never a background one (an engine event re-reads every query), so what
   *  the user confirms is what they saw (D33). A draft that changes on the review (its first
   *  superblock passed) takes its own first preview, and says so. */
  const [reviewed, setReviewed] = useState<{ key: string; p: ProposalPreview } | null>(null);
  const [firstMoved, setFirstMoved] = useState(false);
  /** Pay's re-read found other figures than the review showed: they need a fresh confirm (D33). */
  const [feeChanged, setFeeChanged] = useState(false);
  const mine = useMyProposals(wallet.walletId);
  const [after, setAfter] = useState<AfterFailure | null>(null);
  const credential = useCredential(wallet.walletId, "Enter your passphrase to pay the fee.");

  const choice = useMemo(() => defaultFirstPayment(superblocks.data ?? [], now), [superblocks.data, now]);
  // The default, and again when the chosen superblock passes (the list re-reads on the tip).
  useEffect(() => {
    const gone = first !== null && superblocks.data !== undefined && !superblocks.data.some((s) => s.height === first);
    if ((first === null || gone) && choice.height !== null) setFirst(choice.height);
  }, [first, choice.height, superblocks.data]);

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
  // `preview.data` is always the current key's: gcTime 0 and no placeholder data.
  const draftKey = draft && JSON.stringify(draft);
  useEffect(() => {
    if (!draftKey || !preview.data || reviewed?.key === draftKey) return;
    if (reviewed) setFirstMoved(true);
    setReviewed({ key: draftKey, p: preview.data });
  }, [draftKey, preview.data, reviewed]);
  const figures = reviewed?.key === draftKey ? reviewed.p : undefined;
  /** The draft on screen now, for a press whose awaits outlive it. */
  const shownKey = useRef(draftKey);
  shownKey.current = draftKey;

  /** An engine refusal of the draft's fields: back to the first failing field's step. */
  const refuseFields = (e: unknown): boolean => {
    if (!isAppError(e) || e.code !== "governance.invalid_proposal" || e.data?.kind !== "proposalFields") return false;
    const fields = e.data.fields;
    setErrors(Object.fromEntries(fields.map((f) => [f, fieldText(f, network.kind)])));
    setStep(fields.reduce<Step>((s, f) => Math.min(s, FIELD_STEP[f]) as Step, 3));
    return true;
  };
  useEffect(() => {
    // Read once per failed preview, [Pay]'s re-read included.
    if (step !== 3 || !preview.isError) return;
    if (!refuseFields(preview.error)) setGeneral(governanceErrorText(preview.error) ?? failureText(preview.error, shown.chain, shown.amountUnit));
  }, [preview.isError, preview.error]);

  const next = async () => {
    setGeneral(null);
    setReviewed(null);
    setFirstMoved(false);
    setFeeChanged(false);
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

  /** Whether the failed call made the proposal anyway: Mine says, ignoring the proposals it held
   *  when Pay was pressed (D34). */
  const checkMine = async (d: ProposalDraft, known: ReadonlySet<string>) => {
    setAfter({ kind: "checking" });
    try {
      const made = createdFrom(await b.governanceMyProposals(wallet.walletId), d, known);
      if (made) credential.setError(null); // the notice says what happened instead
      setAfter(made ? { kind: "created", p: made } : null);
    } catch {
      setAfter({ kind: "unknown", known });
    }
    void refresh();
  };

  const create = async () => {
    // The figures the review shows as Pay is pressed: the ones the user confirms.
    const confirmed = figures;
    if (busy || after || !draft || !draftKey || !confirmed || !credential.ready) return;
    const known = new Set((mine.data ?? []).map((p) => p.hash));
    setBusy(true);
    setGeneral(null);
    try {
      const r = await preview.refetch();
      if (r.isError || !r.data) return; // the preview's effect says why
      if (shownKey.current !== draftKey) return; // the draft moved on (D33): its own review
      if (!sameFee(r.data, confirmed)) {
        // The review now shows the new figures; the next press confirms them.
        setReviewed({ key: draftKey, p: r.data });
        setFeeChanged(true);
        return;
      }
      setFeeChanged(false);
      const grant = await credential.authorize({ kind: "spend", maxDuffs: feeGrantCap(confirmed) });
      if (!grant || shownKey.current !== draftKey) return;
      let created: MyProposal;
      try {
        created = await b.governanceCreateProposal(wallet.walletId, draft, grant.id);
      } catch (e) {
        if (refuseFields(e)) return;
        credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
        // `after` keeps Pay and Back off; the sheet may close while Mine is read.
        setBusy(false);
        await checkMine(draft, known);
        return;
      }
      void refresh();
      onCreated(created);
    } catch (e) {
      credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
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
        ) : superblocks.isError && !superblocks.data ? (
          <p className="form-problem" role="alert" data-testid="create-superblocks-error">
            {GovText.superblocksFailed} {governanceErrorText(superblocks.error) ?? failureText(superblocks.error, shown.chain, shown.amountUnit)}
          </p>
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
        {draft && (preview.isPending || (preview.data && !figures)) ? (
          <LoadingState testId="create-preview-loading" text="Checking the proposal…" />
        ) : figures && draft ? (
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
                <AmountText text={shown.full(figures.fee)} unitName={shown.name} /> + network fee{" "}
                <AmountText text={shown.full(figures.networkFee)} unitName={shown.name} />
              </DetailRow>
              <DetailRow label="You pay" testId="create-review-total">
                <AmountText text={shown.full(feeTotal(figures))} unitName={shown.name} />
              </DetailRow>
            </dl>
            {firstMoved && firstSb && (
              <p className="mg-notice mg-notice-warning" role="alert" data-testid="create-first-moved">
                The first payment moved to {shortDate(firstSb.estimatedTime)}: the one you chose has passed. Check the dates before you pay.
              </p>
            )}
            {feeChanged && (
              <p className="mg-notice mg-notice-warning" role="alert" data-testid="create-fee-changed">
                The network fee changed to {shown.withUnit(figures.networkFee)}, so you pay {shown.withUnit(feeTotal(figures))} in all. Nothing was paid: check
                the figures and press Pay again.
              </p>
            )}
            <details className="disclosure card-l1">
              <summary>Proposal data (JSON)</summary>
              <pre className="mono mg-json" data-testid="create-json">
                {figures.dataJson}
              </pre>
            </details>
            <p className="mg-notice mg-notice-warning" data-testid="create-fee-warning">
              <strong>{GovText.feeWarning}</strong> {GovText.feeAfter}
            </p>
            <CredentialField c={credential} busy={busy} testId="create" className="mg-create-credential" />
            {after?.kind === "checking" && <LoadingState testId="create-checking" text="Checking whether the proposal was made…" />}
            {after?.kind === "created" && (
              <div className="mg-notice mg-notice-warning" role="alert" data-testid="create-already-made">
                <p className="mg-flush">
                  <strong>{after.p.name} was made, and its fee may have gone out.</strong> Don&apos;t pay again: Mine shows it and sends it on from there.
                </p>
                <DashButton variant="filledBlue" size="small" data-testid="create-show-mine" onClick={() => onCreated(after.p)}>
                  Show it in Mine
                </DashButton>
              </div>
            )}
            {after?.kind === "unknown" && (
              <div className="mg-notice mg-notice-warning" role="alert" data-testid="create-unknown">
                <p className="mg-flush">Couldn&apos;t check whether the proposal was made, so Pay stays off: its fee may have gone out.</p>
                <DashButton variant="filledBlue" size="small" data-testid="create-check-again" onClick={() => void checkMine(draft, after.known)}>
                  Check again
                </DashButton>
              </div>
            )}
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
              <DashButton variant="tintedGray" size="medium" data-testid="create-back" disabled={busy || after !== null} onClick={() => setStep((s) => (s - 1) as Step)}>
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
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="create-pay" disabled={busy || after !== null || !figures || !credential.ready}>
                {busy ? "Paying…" : `Pay ${shown.compact(figures?.fee ?? 100_000_000)} ${shown.name} and create`}
              </DashButton>
            )}
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
