// Create a proposal (§4.25.3, D14, MG-06): Details, Payments, then Review and pay the fee. Review
// reads `preview_proposal`; [Pay 1 Dash and create] reads it again (the coins may have moved): a
// changed fee stops for a fresh confirm (D33), the same one asks a spend grant capped at the
// confirmed total plus dwcli's margin (D29), then `create_proposal`. An engine field refusal goes
// back to the first failing field's step. Any other failure may come after the fee went out, so
// Pay waits for a re-read of Mine and, if the proposal is there, sends the user to it (D34). Once
// paid, the proposal is the engine's (Mine shows it and resumes it after a restart, D27).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { isAppError } from "../../api/backend";
import { lockGeneration, refusedWhileLocked, walletGateClosed } from "../../api/lockGate";
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
  inProgressRefusal,
  lastPaymentText,
  MAX_PAYMENTS,
  sameFee,
  shortAddress,
  shortDate,
  superblockOption,
} from "../../governance/model";
import { useGovernanceInfo, useGovernanceRefresh, useMyProposals, useUpcomingSuperblocks } from "../../governance/queries";
import { useT } from "../../i18n";
import { useAmountField } from "../pay/form";
import { failureText } from "../pay/model";
import { useNowSecs } from "./common";
import { CredentialField, useCredential } from "./credential";

type Step = 1 | 2 | 3;
const STEPS = [
  { n: 1, title: "proposal", heading: "proposalHeading" },
  { n: 2, title: "payments", heading: "paymentsHeading" },
  { n: 3, title: "review", heading: "reviewHeading" },
] as const satisfies readonly { n: Step; title: string; heading: string }[];

/** The proposal fee as the warning names it (funds copy, MG-00 D16). */
const PROPOSAL_FEE = "1 DASH";

type FieldErrors = Partial<Record<ProposalFieldName, string>>;

/** After a failed `create_proposal` (D34): reading Mine, the proposal it made anyway, Mine
 *  unreadable, or a refusal: the proposal of that name already in progress, or one
 *  whose payment is under way. Pay stays off in all five. */
type AfterFailure =
  | { kind: "checking" }
  | { kind: "created"; p: MyProposal }
  | { kind: "unknown"; known: ReadonlySet<string> }
  | { kind: "existing"; p: MyProposal }
  | { kind: "underway" };

export function CreateProposalSheet({
  wallet,
  network,
  onClose,
  onCreated,
  onShowMine,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  onClose: () => void;
  onCreated: (p: MyProposal) => void;
  /** A refused draft's way to the proposal already in progress: Mine, with no result to announce. */
  onShowMine: () => void;
}) {
  const t = useT();
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
  const amountPending = !amount.settled || (amount.text.trim() !== "" && amount.duffs === undefined);
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
  /** The engine refused the draft before anything was paid. */
  const refused = after?.kind === "existing" || after?.kind === "underway";

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
  // Unknown figures ask. The grant is the submission fee, not the proposed payout.
  const credential = useCredential(
    wallet.walletId,
    t(`masternodes.governance.createSheet.ask`),
    figures ? feeGrantCap(figures) : null,
  );
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
      if (amountPending) return;
      const p: FieldErrors = {};
      if (first === null) p.first_payment = fieldText("first_payment", network.kind);
      if (amount.error || !amount.duffs) p.payment_amount = amount.error ?? t("masternodes.governance.field.amountEmpty");
      if (payTo === "other" && other.trim() === "") p.payment_address = fieldText("payment_address", network.kind);
      setErrors(p);
      if (Object.keys(p).length > 0) return;
      if (payTo === "new" && fresh === null) {
        try {
          setFresh((await b.nextReceiveAddress(wallet.walletId, t("masternodes.governance.createSheet.addressLabel", { name }))).address);
        } catch (e) {
          setGeneral(failureText(e, shown.chain, shown.amountUnit));
          return;
        }
      }
      setStep(3);
    }
  };

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /** Whether the failed call made the proposal anyway: Mine says, ignoring the proposals it held
   *  when Pay was pressed (D34). */
  const checkMine = async (d: ProposalDraft, known: ReadonlySet<string>) => {
    setAfter({ kind: "checking" });
    let next: AfterFailure | null;
    try {
      const made = createdFrom(await b.governanceMyProposals(wallet.walletId), d, known);
      if (made && mounted.current) credential.setError(null); // the notice says what happened instead
      next = made ? { kind: "created", p: made } : null;
    } catch {
      next = { kind: "unknown", known };
    }
    if (mounted.current) setAfter(next);
    void refresh();
  };

  /** The engine refused the draft: a proposal of its name is in progress. Mine's row of it, or
   *  "under way" when the refusal names none or Mine can't be read. */
  const showExisting = async (hash: string | null) => {
    setAfter({ kind: "checking" });
    let next: AfterFailure;
    try {
      const p = hash === null ? undefined : (await b.governanceMyProposals(wallet.walletId)).find((m) => m.hash === hash);
      next = p ? { kind: "existing", p } : { kind: "underway" };
    } catch {
      next = { kind: "underway" };
    }
    if (mounted.current) setAfter(next);
    void refresh();
  };

  const create = async () => {
    // The figures the review shows as Pay is pressed: the ones the user confirms.
    const confirmed = figures;
    if (busy || after || !draft || !draftKey || !confirmed || !credential.ready) return;
    const known = new Set((mine.data ?? []).map((p) => p.hash));
    // Once the sheet closes or a lock comes, nothing this press started may pay.
    const gen = lockGeneration();
    const departed = () => !mounted.current || walletGateClosed() || lockGeneration() !== gen;
    setBusy(true);
    setGeneral(null);
    try {
      const r = await preview.refetch();
      if (departed()) return;
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
      if (!grant) return;
      if (departed() || shownKey.current !== draftKey) {
        void b.revokeGrant(grant.id).catch(() => null);
        return;
      }
      let created: MyProposal;
      try {
        created = await b.governanceCreateProposal(wallet.walletId, draft, grant.id);
      } catch (e) {
        // A call the lock stopped before the engine paid nothing; a sheet that has gone shows and
        // reads nothing more.
        if (refusedWhileLocked(e) || !mounted.current || refuseFields(e)) return;
        const existing = inProgressRefusal(e);
        if (existing !== undefined) {
          setBusy(false);
          await showExisting(existing);
          return;
        }
        credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
        // `after` keeps Pay and Back off; the sheet may close while Mine is read.
        // (A refusal, above, keeps Pay off and leaves Back on.)
        setBusy(false);
        await checkMine(draft, known);
        return;
      }
      void refresh();
      onCreated(created);
    } catch (e) {
      if (mounted.current) credential.setError(governanceErrorText(e) ?? failureText(e, shown.chain, shown.amountUnit));
    } finally {
      if (mounted.current) setBusy(false);
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
  const strong = (children: ReactNode) => <strong>{children}</strong>;
  const paidTo = draft && (
    <span className="mono" title={draft.paymentAddress}>
      {shortAddress(draft.paymentAddress)}
    </span>
  );

  let body;
  if (step === 1)
    body = (
      <div className="mg-form">
        <label className={`mg-field mg-span ${errors.name ? "has-error" : ""}`}>
          {t("masternodes.governance.createSheet.name")}
          <input data-testid="create-name" className="mono" value={name} maxLength={40} autoComplete="off" data-autofocus onChange={(e) => setName(e.target.value)} {...fieldProps("name")} />
          <span className="mg-field-help">{GovText.nameHelp}</span>
          {fieldError("name")}
        </label>
        <label className={`mg-field mg-span ${errors.url ? "has-error" : ""}`}>
          {t("masternodes.governance.createSheet.link")}
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
          <LoadingState testId="create-superblocks" text={t("masternodes.governance.createSheet.readingSuperblocks")} />
        ) : superblocks.isError && !superblocks.data ? (
          <p className="form-problem" role="alert" data-testid="create-superblocks-error">
            {GovText.superblocksFailed} {governanceErrorText(superblocks.error) ?? failureText(superblocks.error, shown.chain, shown.amountUnit)}
          </p>
        ) : (
          <div className="mg-form">
            <label className="mg-field">
              {t("masternodes.governance.createSheet.firstPayment")}
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
              {t("masternodes.governance.createSheet.count")}
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
              {t("masternodes.governance.createSheet.amount")}
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
              {t("masternodes.governance.createSheet.total")}
              <span className="mg-field-row mg-total" data-testid="create-total">
                {total !== null ? shown.withUnit(total) : "—"}
              </span>
            </div>
            <label className={`mg-field mg-span ${errors.payment_address ? "has-error" : ""}`}>
              {t("masternodes.governance.createSheet.payTo")}
              <select data-testid="create-pay-to" value={payTo} onChange={(e) => setPayTo(e.target.value as "new" | "other")}>
                <option value="new">{t("masternodes.governance.createSheet.newAddress")}</option>
                <option value="other">{t("masternodes.governance.createSheet.otherAddress")}</option>
              </select>
              {payTo === "other" && (
                <input
                  data-testid="create-address"
                  className="mono"
                  aria-label={t("masternodes.governance.createSheet.addressToPay")}
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
          <LoadingState testId="create-preview-loading" text={t("masternodes.governance.createSheet.checking")} />
        ) : figures && draft ? (
          <>
            <dl className="detail-list card-l1" data-testid="create-review">
              <DetailRow label={t("masternodes.governance.createSheet.name")}>
                <span className="mono">{draft.name}</span>
              </DetailRow>
              <DetailRow label={t("masternodes.governance.createSheet.link")}>{draft.url}</DetailRow>
              <DetailRow label={t("masternodes.governance.createSheet.paymentsLabel")} testId="create-review-payments">
                {t.rich(
                  firstSb
                    ? lastSb && draft.paymentCount > 1
                      ? "masternodes.governance.createSheet.reviewPaymentsSpan"
                      : "masternodes.governance.createSheet.reviewPaymentsFrom"
                    : lastSb && draft.paymentCount > 1
                      ? "masternodes.governance.createSheet.reviewPaymentsTo"
                      : "masternodes.governance.createSheet.reviewPayments",
                  {
                    amount: <AmountText text={shown.compact(draft.paymentAmount)} unitName={shown.name} />,
                    count: String(draft.paymentCount),
                    first: firstSb ? shortDate(firstSb.estimatedTime) : "",
                    last: lastSb ? shortDate(lastSb.estimatedTime) : "",
                    total: <AmountText text={shown.compact(draft.paymentAmount * draft.paymentCount)} unitName={shown.name} />,
                  },
                )}
              </DetailRow>
              <DetailRow label={t("masternodes.governance.createSheet.paidTo")}>
                {payTo === "new" ? t.rich("masternodes.governance.createSheet.thisWallet", { address: paidTo }) : paidTo}
              </DetailRow>
              <DetailRow label={t("masternodes.governance.createSheet.fee")} testId="create-review-fee">
                {t.rich("masternodes.governance.createSheet.feeValue", {
                  fee: <AmountText text={shown.full(figures.fee)} unitName={shown.name} />,
                  networkFee: <AmountText text={shown.full(figures.networkFee)} unitName={shown.name} />,
                })}
              </DetailRow>
              <DetailRow label={t("masternodes.governance.createSheet.youPay")} testId="create-review-total">
                <AmountText text={shown.full(feeTotal(figures))} unitName={shown.name} />
              </DetailRow>
            </dl>
            {firstMoved && firstSb && (
              <p className="mg-notice mg-notice-warning" role="alert" data-testid="create-first-moved">
                {t("masternodes.governance.createSheet.firstMoved", { date: shortDate(firstSb.estimatedTime) })}
              </p>
            )}
            {feeChanged && (
              <p className="mg-notice mg-notice-warning" role="alert" data-testid="create-fee-changed">
                {t("masternodes.governance.createSheet.feeChanged", { networkFee: shown.withUnit(figures.networkFee), total: shown.withUnit(feeTotal(figures)) })}
              </p>
            )}
            <details className="disclosure card-l1">
              <summary>{t("masternodes.governance.createSheet.json")}</summary>
              <pre className="mono mg-json" data-testid="create-json">
                {figures.dataJson}
              </pre>
            </details>
            <p className="mg-notice mg-notice-warning" data-testid="create-fee-warning">
              <strong>{t("masternodes.governance.feeWarning", { fee: PROPOSAL_FEE })}</strong> {GovText.feeAfter}
            </p>
            <CredentialField c={credential} busy={busy} testId="create" className="mg-create-credential" />
            {after?.kind === "checking" && <LoadingState testId="create-checking" text={t("masternodes.governance.createSheet.checkingMade")} />}
            {after?.kind === "created" && (
              <div className="mg-notice mg-notice-warning" role="alert" data-testid="create-already-made">
                <p className="mg-flush">{t.rich("masternodes.governance.createSheet.made", { name: after.p.name, strong })}</p>
                <DashButton variant="filledBlue" size="small" data-testid="create-show-mine" onClick={() => onCreated(after.p)}>
                  {t("masternodes.governance.createSheet.showInMine")}
                </DashButton>
              </div>
            )}
            {after?.kind === "existing" && (
              <div className="mg-notice mg-notice-warning" role="alert" data-testid="create-in-progress">
                <p className="mg-flush">{t.rich("masternodes.governance.createSheet.inProgress", { name: after.p.name, strong })}</p>
                <DashButton variant="filledBlue" size="small" data-testid="create-show-existing" onClick={onShowMine}>
                  {t("masternodes.governance.createSheet.showInMine")}
                </DashButton>
              </div>
            )}
            {after?.kind === "underway" && (
              <p className="mg-notice mg-notice-warning" role="alert" data-testid="create-underway">
                {t("masternodes.governance.createSheet.underway", { name: draft.name })}
              </p>
            )}
            {after?.kind === "unknown" && (
              <div className="mg-notice mg-notice-warning" role="alert" data-testid="create-unknown">
                <p className="mg-flush">{t("masternodes.governance.createSheet.unknown")}</p>
                <DashButton variant="filledBlue" size="small" data-testid="create-check-again" onClick={() => void checkMine(draft, after.known)}>
                  {t("masternodes.governance.createSheet.checkAgain")}
                </DashButton>
              </div>
            )}
          </>
        ) : null}
      </>
    );

  const current = STEPS.find((s) => s.n === step)!;
  return (
    <Sheet title={t("masternodes.governance.createSheet.title")} width="large" testId="create-sheet" closable={!busy} focusKey={String(step)} onClose={onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void (step === 3 ? create() : next());
        }}
      >
        <div className="mg-wizard">
          <ol className="mg-steps" aria-label={t("masternodes.governance.createSheet.steps")}>
            {STEPS.map((s) => (
              <li key={s.n} data-n={s.n} className={s.n < step ? "is-done" : undefined} aria-current={s.n === step ? "step" : undefined}>
                {t(`masternodes.governance.createSheet.${s.title}`)}
              </li>
            ))}
          </ol>
          <div className="mg-step" data-testid={`create-step-${step}`}>
            <h3 className="mg-step-title">{t(`masternodes.governance.createSheet.${current.heading}`)}</h3>
            {body}
            <p className="form-problem" role="alert" data-testid="create-general">
              {general}
            </p>
          </div>
        </div>
        <footer className="mg-sheet-footer">
          <span className="mg-sheet-footer-note">
            {t("masternodes.governance.createSheet.stepOf", { step: String(current.n), steps: String(STEPS.length) })}
          </span>
          <span className="mg-actions">
            {step > 1 ? (
              <DashButton
                variant="tintedGray"
                size="medium"
                data-testid="create-back"
                disabled={busy || (after !== null && !refused)}
                onClick={() => {
                  // A refusal paid nothing: the user may go back and rename, and Pay comes back.
                  if (refused) setAfter(null);
                  setStep((s) => (s - 1) as Step);
                }}
              >
                {t("masternodes.governance.createSheet.back")}
              </DashButton>
            ) : (
              <DashButton variant="tintedGray" size="medium" data-testid="create-cancel" onClick={onClose}>
                {t("common.cancel")}
              </DashButton>
            )}
            {step < 3 ? (
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="create-continue" disabled={step === 2 && amountPending}>
                {t("masternodes.governance.createSheet.continue")}
              </DashButton>
            ) : (
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="create-pay" disabled={busy || after !== null || !figures || !credential.ready}>
                {busy ? t("masternodes.governance.createSheet.paying") : t("masternodes.governance.createSheet.pay", { fee: `${shown.compact(figures?.fee ?? 100_000_000)} ${shown.name}` })}
              </DashButton>
            )}
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
