// Set up a masternode (MG-10 §1.6, MG-00 `reg-*`): Type, Collateral, Operator, Keys and payout,
// Review; then Prove for a collateral held in another wallet, and Sent. Review reads
// `preview_registration`; [Register] reads it again (D5): changed figures show again and the next
// press confirms them; the same ones ask a `masternodeOp` grant capped at the confirmed total plus
// the margin. An External registration is saved at Review's [Continue] (`needs_signature`, In
// progress) and reopens at Prove. A refusal goes back to the step and field it belongs to.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { isAppError } from "../../api/backend";
import { lockGeneration, refusedWhileLocked, walletGateClosed } from "../../api/lockGate";
import type { CollateralCoin } from "../../api/generated/CollateralCoin";
import type { MnRegistrationPreview } from "../../api/generated/MnRegistrationPreview";
import type { MasternodeTypeName } from "../../api/generated/MasternodeTypeName";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { NetworkKind } from "../../api/generated/NetworkKind";
import type { ProTxKeys } from "../../api/generated/ProTxKeys";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { CopyIconButton } from "../../components/copy";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { copyWithToast } from "../../components/toast";
import { useDisplay } from "../../format/display";
import { shortAddress, shortHash } from "../../governance/model";
import { useT } from "../../i18n";
import { mnErrorText } from "../../masternodes/model";
import { lastErrorText } from "../../masternodes/operations";
import { mnKeys, useMasternodes, useMasternodesRefresh, useRegistrationKeys } from "../../masternodes/queries";
import {
  buildRequest,
  coinsFor,
  collateralDash,
  newDraft,
  refusalOf,
  registerCap,
  samePreview,
  stepErrors,
  STEPS,
  type Field,
  type FieldErrors,
  type Ports,
  type RegDraft,
  type Step,
  collateralNote,
  finalAction,
  registrationCalls,
  registrationSends,
} from "../../masternodes/registration";
import { failureText } from "../pay/model";
import { CredentialField, useCredential } from "./credential";

const R = "masternodes.register";

/** The masternode port of each network, and the one an address from another network would carry. */
const PORT: Record<NetworkKind, number> = { mainnet: 9999, testnet: 19999, devnet: 19799, regtest: 19899 };

export function portsOf(kind: NetworkKind, name: (k: NetworkKind) => string): Ports {
  const other: NetworkKind | null = kind === "mainnet" ? "testnet" : kind === "testnet" ? "mainnet" : null;
  return { own: PORT[kind], other: other ? { port: PORT[other], network: name(other) } : null, network: name(kind) };
}

export function RegisterSheet({
  wallet,
  network,
  resume,
  onClose,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  /** A saved External registration: opens at Prove (In progress ▸ Continue / Get a new message). */
  resume?: ProTxOperation;
  onClose: () => void;
}) {
  const t = useT();
  const backend = useBackend();
  const sends = registrationSends(network);
  const b = registrationCalls(backend, sends);
  const shown = useDisplay(network);
  const id = useId();
  const refresh = useMasternodesRefresh();
  const queryClient = useQueryClient();
  const overview = useMasternodes(wallet.walletId);
  const [step, setStep] = useState<Step>(resume ? "prove" : "type");
  const [d, setD] = useState<RegDraft>(newDraft);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [general, setGeneral] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [askDiscard, setAskDiscard] = useState(false);
  /** The figures the review shows: the first read, then only Register's own re-read moves them (D5). */
  const [reviewed, setReviewed] = useState<{ key: string; p: MnRegistrationPreview } | null>(null);
  const [changed, setChanged] = useState(false);
  /** The saved operation: External from Prepare (or `resume`), or what Register returned. */
  const [op, setOp] = useState<ProTxOperation | null>(resume ?? null);
  const [signature, setSignature] = useState("");
  const [inputsSpent, setInputsSpent] = useState(resume?.state.kind === "inputs_spent");
  // Register is with the network: its own operation already holds the collateral and keys, so a keys
  // or preview read now would hand out the next keys and refuse the coin this send is using (issue 595).
  // The keys are held from the call itself, since the send announces its change before React renders.
  const [sending, setSending] = useState(false);
  const sendingKeys = useRef<ProTxKeys | null>(null);
  // The engine sends a sign request only while the row needs a signature; a spent row needs a new message.
  const needsNewMessage = inputsSpent || (op !== null && op.signRequest === null);
  const set = (patch: Partial<RegDraft>) => setD((x) => ({ ...x, ...patch }));

  const keys = useRegistrationKeys(wallet.walletId, d.kind, !resume && step !== "type" && step !== "collateral" && !sending);
  const coins = useMemo(() => coinsFor(overview.data?.collateralCoins ?? [], d.kind), [overview.data, d.kind]);
  const allCoins: CollateralCoin[] = overview.data?.collateralCoins ?? [];
  const ports = portsOf(network.kind, (k) => t(`network.${k}`));
  const external = d.collateral === "external";
  const keyData = sendingKeys.current ?? keys.data;
  const request = useMemo(() => buildRequest(d, keyData), [d, keyData]);
  const requestKey = JSON.stringify(request);
  const preview = useQuery({
    queryKey: ["masternodes", "registrationPreview", wallet.walletId, requestKey] as const,
    queryFn: () => b.mnPreviewRegistration(wallet.walletId, request),
    enabled: step === "review" && keys.isSuccess && !sending,
    retry: false,
    gcTime: 0,
  });
  useEffect(() => {
    if (step !== "review" || !preview.data || reviewed?.key === requestKey) return;
    setReviewed({ key: requestKey, p: preview.data });
  }, [step, preview.data, requestKey, reviewed]);
  const figures = reviewed?.key === requestKey ? reviewed.p : undefined;

  const refuse = (e: unknown): boolean => {
    const r = refusalOf(e, d.kind);
    if (!r) return false;
    if (r.field) setErrors({ [r.field]: r.text });
    else setGeneral(r.text);
    if (r.step !== step) setStep(r.step);
    return true;
  };
  const errorText = (e: unknown) => mnErrorText(e) === (isAppError(e) ? e.message : String(e)) ? failureText(e, shown.chain, shown.amountUnit) : mnErrorText(e);
  useEffect(() => {
    if (step !== "review" || sendingKeys.current || !preview.isError) return;
    if (!refuse(preview.error)) setGeneral(errorText(preview.error));
  }, [preview.isError, preview.error]);
  useEffect(() => {
    if (!keys.isError) return;
    if (!refuse(keys.error)) setGeneral(errorText(keys.error));
  }, [keys.isError, keys.error]);

  const cap = op && step === "prove" ? op.debit : op && op.state.kind === "not_sent" ? op.debit : figures ? registerCap(figures) : null;
  const credential = useCredential(wallet.walletId, t(`${R}.review.ask`), cap, sends);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const shownKey = useRef(requestKey);
  shownKey.current = requestKey;

  const index = STEPS.indexOf(step as (typeof STEPS)[number]);
  const goBack = () => {
    setGeneral(null);
    setErrors({});
    setChanged(false);
    setStep(step === "prove" ? "review" : STEPS[Math.max(0, index - 1)]);
  };

  const next = async () => {
    setGeneral(null);
    if (step === "review") return external ? prepare() : register();
    const e = stepErrors(step, d, ports, allCoins, keys.data?.owner.address ?? null);
    setErrors(e);
    if (Object.keys(e).length > 0) return;
    setReviewed(null);
    setChanged(false);
    setStep(STEPS[index + 1]);
  };

  /** One press's guard: the sheet closed or a lock came, so nothing started may sign. */
  const pressGuard = () => {
    const gen = lockGeneration();
    return () => !mounted.current || walletGateClosed() || lockGeneration() !== gen;
  };

  const prepare = async () => {
    if (busy || !figures) return;
    setBusy(true);
    try {
      const saved = await b.mnPrepareExternalRegistration(wallet.walletId, request);
      if (!mounted.current) return;
      setOp(saved);
      setInputsSpent(false);
      setStep("prove");
      void refresh();
    } catch (e) {
      if (mounted.current && !refuse(e)) setGeneral(errorText(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const register = async () => {
    const confirmed = figures;
    if (busy || !confirmed || !credential.ready) return;
    const departed = pressGuard();
    setBusy(true);
    let granted: string | null = null;
    try {
      const r = await preview.refetch();
      if (departed() || r.isError || !r.data || shownKey.current !== requestKey) return;
      if (!samePreview(r.data, confirmed)) {
        setReviewed({ key: requestKey, p: r.data });
        setChanged(true);
        return;
      }
      setChanged(false);
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: registerCap(confirmed) });
      if (!grant) return;
      granted = grant.id;
      if (departed()) {
        void backend.revokeGrant(grant.id).catch(() => null);
        return;
      }
      sendingKeys.current = keys.data ?? null;
      setSending(true);
      const sent = await b.mnRegister(wallet.walletId, request, grant.id);
      void refresh();
      if (!mounted.current) return;
      setOp(sent);
      setStep("done");
    } catch (e) {
      if (granted) void backend.revokeGrant(granted).catch(() => null);
      if (refusedWhileLocked(e) || !mounted.current) return;
      if (isAppError(e) && e.code === "send.grant_exceeded") {
        setChanged(true);
        void preview.refetch().then((r) => r.data && mounted.current && setReviewed({ key: requestKey, p: r.data }));
      }
      if (!refuse(e)) credential.setError(errorText(e));
    } finally {
      sendingKeys.current = null;
      if (mounted.current) {
        setSending(false);
        setBusy(false);
      }
    }
  };

  /** Prove's [Register], and Sent's [Try again]: a grant capped at the saved operation's `debit`. */
  const submitOrResend = async () => {
    if (busy || !op || !credential.ready) return;
    if (step === "prove" && signature.trim() === "") {
      setErrors({ signature: t(`${R}.prove.signatureHelp`) });
      return;
    }
    const departed = pressGuard();
    setBusy(true);
    setErrors({});
    let granted: string | null = null;
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: op.debit });
      if (!grant) return;
      granted = grant.id;
      if (departed()) {
        void backend.revokeGrant(grant.id).catch(() => null);
        return;
      }
      const result =
        step === "prove"
          ? await b.mnSubmitExternalRegistration(wallet.walletId, op.id, signature.trim(), grant.id)
          : await b.mnResendOperation(wallet.walletId, op.id, grant.id);
      void refresh();
      if (!mounted.current) return;
      setOp(result);
      setStep("done");
    } catch (e) {
      if (granted) void backend.revokeGrant(granted).catch(() => null);
      if (refusedWhileLocked(e) || !mounted.current) return;
      if (isAppError(e) && e.code === "masternodes.operation_state" && e.data?.kind === "operationState" && e.data.state === "inputs_spent") {
        setInputsSpent(true);
        // The saved row reads spent at once, so In progress offers a new message if this sheet closes
        // before the list is read again.
        queryClient.setQueryData<ProTxOperation[]>(mnKeys.operations(wallet.walletId), (rows) =>
          rows?.map((o) => (o.id === op.id ? { ...o, state: { kind: "inputs_spent" }, signRequest: null } : o)),
        );
        void refresh();
        return;
      }
      if (!refuse(e)) credential.setError(errorText(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const newMessage = async () => {
    if (busy || !op) return;
    setGeneral(null);
    setBusy(true);
    try {
      const fresh = await b.mnRefreshExternalRegistration(wallet.walletId, op.id);
      // In progress reopens the saved row as the engine returned it, even before the list is read again.
      queryClient.setQueryData<ProTxOperation[]>(mnKeys.operations(wallet.walletId), (rows) => rows?.map((o) => (o.id === fresh.id ? fresh : o)));
      if (!mounted.current) return;
      setOp(fresh);
      setSignature("");
      setInputsSpent(false);
      void refresh();
    } catch (e) {
      if (mounted.current) setGeneral(errorText(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const close = () => {
    if (busy) return;
    // Nothing was sent before Prove; a prepared External is saved, and Sent is done.
    if (step !== "type" && step !== "prove" && step !== "done" && !askDiscard) return setAskDiscard(true);
    onClose();
  };

  const fieldProps = (f: Field) => ({
    "aria-invalid": errors[f] ? true : undefined,
    "aria-describedby": errors[f] ? `${id}-${f}-error` : undefined,
  });
  const fieldError = (f: Field) =>
    errors[f] && (
      <span className="mg-field-error" id={`${id}-${f}-error`} data-testid={`reg-error-${f}`}>
        {errors[f]}
      </span>
    );
  const amount = (duffs: number) => <AmountText text={shown.full(duffs)} unitName={shown.name} />;
  const dash = collateralDash(d.kind);
  const choice = (name: string, value: string, checked: boolean, onPick: () => void, title: ReactNode, text: ReactNode, testId: string, disabled = false) => (
    <label className="mg-choice" data-testid={testId}>
      <input type="radio" name={`${id}-${name}`} value={value} checked={checked} disabled={disabled} onChange={onPick} />
      <span className="mg-choice-text">
        <strong>{title}</strong>
        <span className="mg-small mg-muted">{text}</span>
      </span>
    </label>
  );

  let body: ReactNode = null;
  if (step === "type")
    body = (
      <>
        <div className="mg-choices" role="radiogroup" aria-labelledby={`${id}-reg-kind`} data-testid="reg-kind">
          <span id={`${id}-reg-kind`} className="mg-section-title">{t(`${R}.type.what`)}</span>
          {choice("kind", "regular", d.kind === "regular", () => set({ kind: "regular", outpoint: "" }), t(`${R}.type.regular`), t(`${R}.type.regularText`, { amount: 1000 }), "reg-kind-regular")}
          {choice("kind", "evonode", d.kind === "evonode", () => set({ kind: "evonode", outpoint: "" }), t(`${R}.type.evonode`), t(`${R}.type.evonodeText`, { amount: 4000 }), "reg-kind-evonode")}
        </div>
        <div className="mg-choices" role="radiogroup" aria-labelledby={`${id}-reg-runner`} data-testid="reg-runner">
          <span id={`${id}-reg-runner`} className="mg-section-title">{t(`${R}.type.who`)}</span>
          {choice("runner", "hosted", d.runner === "hosted", () => set({ runner: "hosted" }), t(`${R}.type.hosted`), t(`${R}.type.hostedText`), "reg-runner-hosted")}
          {choice("runner", "self", d.runner === "self", () => set({ runner: "self", operatorFromWallet: true, nodeFromWallet: true }), t(`${R}.type.self`), t(`${R}.type.selfText`), "reg-runner-self")}
        </div>
      </>
    );
  else if (step === "collateral")
    body = (
      <>
        <p id={`${id}-reg-collateral`} className="mg-small">{t(`${R}.collateral.intro`)}</p>
        <div className="mg-choices" role="radiogroup" aria-labelledby={`${id}-reg-collateral`} data-testid="reg-collateral">
          {choice(
            "collateral",
            "fund_new",
            d.collateral === "fund_new",
            () => set({ collateral: "fund_new" }),
            t(`${R}.collateral.fundNew`),
            <>
              {t(`${R}.collateral.fundNewText`, { amount: dash })}{" "}
              {wallet.balances && t.rich(`${R}.collateral.available`, { amount: amount(wallet.balances.confirmed) })}
            </>,
            "reg-collateral-fund-new",
          )}
          {choice("collateral", "existing", d.collateral === "existing", () => set({ collateral: "existing" }), t(`${R}.collateral.existing`, { amount: dash }), t(`${R}.collateral.existingText`, { amount: dash }), "reg-collateral-existing")}
          {d.collateral === "existing" && (
            <div className="mg-coins" data-testid="reg-coins" {...fieldProps("outpoint")}>
              {coins.length === 0 ? (
                <p className="mg-small mg-muted" data-testid="reg-no-coins">
                  {t(`${R}.collateral.noCoins`, { amount: dash })}
                </p>
              ) : (
                coins.map((c) => (
                  <label key={c.outpoint} className="mg-choice" data-testid={`reg-coin-${c.outpoint}`}>
                    <input type="radio" name={`${id}-coin`} value={c.outpoint} checked={d.outpoint === c.outpoint} disabled={c.userLocked} onChange={() => set({ outpoint: c.outpoint })} />
                    <span className="mg-choice-text">
                      <span className="mono">
                        {shortHash(c.outpoint.split("-")[0])}:{c.outpoint.split("-")[1]} · {shortAddress(c.address)}
                      </span>
                      <span className="mg-small mg-muted">{c.userLocked ? t(`${R}.collateral.locked`) : t(`${R}.collateral.confirmations`, { count: c.confirmations })}</span>
                    </span>
                  </label>
                ))
              )}
              {fieldError("outpoint")}
            </div>
          )}
          {choice("collateral", "external", external, () => set({ collateral: "external" }), t(`${R}.collateral.external`), t(`${R}.collateral.externalText`), "reg-collateral-external")}
        </div>
        {external && (
          <div className="mg-form">
            <label className={`mg-field mg-span ${errors.externalOutpoint ? "has-error" : ""}`}>
              {t(`${R}.collateral.outpoint`)}
              <input data-testid="reg-external-outpoint" className="mono" value={d.externalOutpoint} autoComplete="off" spellCheck={false} onChange={(e) => set({ externalOutpoint: e.target.value })} {...fieldProps("externalOutpoint")} />
              <span className="mg-field-help">{t(`${R}.collateral.outpointHelp`)}</span>
              {fieldError("externalOutpoint")}
            </label>
            <label className={`mg-field mg-span ${errors.externalAddress ? "has-error" : ""}`}>
              {t(`${R}.collateral.address`)}
              <input data-testid="reg-external-address" className="mono" value={d.externalAddress} autoComplete="off" spellCheck={false} onChange={(e) => set({ externalAddress: e.target.value })} {...fieldProps("externalAddress")} />
              <span className="mg-field-help">{t(`${R}.collateral.addressHelp`)}</span>
              {fieldError("externalAddress")}
            </label>
          </div>
        )}
      </>
    );
  else if (step === "operator") {
    const self = d.runner === "self";
    const evo = d.kind === "evonode";
    const k = keys.data;
    const walletOperator = self && d.operatorFromWallet && k;
    const intro = !self
      ? t(`${R}.operator.hostedIntro`)
      : walletOperator && evo && d.nodeFromWallet && k.platformNode
        ? t(`${R}.operator.selfIntroEvo`, { index: String(k.operator.index + 1), node: String(k.platformNode.index + 1) })
        : walletOperator
          ? t(`${R}.operator.selfIntro`, { index: String(k.operator.index + 1) })
          : null;
    body = (
      <>
        {intro && <p className="mg-small">{intro}</p>}
        <div className="mg-form">
          <label className={`mg-field mg-span ${errors.operatorKey ? "has-error" : ""}`}>
            {t(`${R}.operator.key`)}
            {self && d.operatorFromWallet ? (
              <span className="mg-field-row">
                <span className="mono mg-key" data-testid="reg-operator-wallet-key">
                  {k?.operator.publicKey ? shortHash(k.operator.publicKey) : "…"}
                </span>
                <DashButton variant="plainBlue" size="small" data-testid="reg-operator-enter" onClick={() => set({ operatorFromWallet: false })}>
                  {t(`${R}.operator.enterInstead`)}
                </DashButton>
              </span>
            ) : (
              <>
                <input data-testid="reg-operator-key" className="mono" value={d.operatorKey} autoComplete="off" spellCheck={false} data-autofocus onChange={(e) => set({ operatorKey: e.target.value })} {...fieldProps("operatorKey")} />
                {self && (
                  <DashButton variant="plainBlue" size="small" data-testid="reg-operator-use-wallet" onClick={() => set({ operatorFromWallet: true, operatorKey: "" })}>
                    {t(`${R}.operator.useWallet`)}
                  </DashButton>
                )}
              </>
            )}
            <span className="mg-field-help">{t(`${R}.operator.keyHelp`)}</span>
            {fieldError("operatorKey")}
          </label>
          <label className={`mg-field ${errors.service ? "has-error" : ""}`}>
            {t(`${R}.operator.service`)}
            <input data-testid="reg-service" className="mono" value={d.service} placeholder={`203.0.113.7:${ports.own}`} autoComplete="off" spellCheck={false} onChange={(e) => set({ service: e.target.value })} {...fieldProps("service")} />
            <span className="mg-field-help">{self ? t(`${R}.operator.serviceHelpSelf`, { port: String(ports.own) }) : t(`${R}.operator.serviceHelp`)}</span>
            {fieldError("service")}
          </label>
          <label className={`mg-field ${errors.operatorShare ? "has-error" : ""}`}>
            {t(`${R}.operator.share`)}
            <span className="mg-field-row">
              <input data-testid="reg-share" inputMode="decimal" value={d.operatorShare} onChange={(e) => set({ operatorShare: e.target.value })} {...fieldProps("operatorShare")} />
              <span>%</span>
            </span>
            <span className="mg-field-help">{t(`${R}.operator.shareHelp`)}</span>
            {fieldError("operatorShare")}
          </label>
          {evo && (
            <>
              <label className={`mg-field mg-span ${errors.nodeId ? "has-error" : ""}`}>
                {t(`${R}.operator.nodeId`)}
                {self && d.nodeFromWallet ? (
                  <span className="mg-field-row">
                    <span className="mono mg-key" data-testid="reg-node-wallet">
                      {k?.platformNode?.nodeId ?? "…"}
                    </span>
                    <DashButton variant="plainBlue" size="small" data-testid="reg-node-enter" onClick={() => set({ nodeFromWallet: false })}>
                      {t(`${R}.operator.enterInstead`)}
                    </DashButton>
                  </span>
                ) : (
                  <input data-testid="reg-node-id" className="mono" value={d.nodeId} autoComplete="off" spellCheck={false} onChange={(e) => set({ nodeId: e.target.value })} {...fieldProps("nodeId")} />
                )}
                <span className="mg-field-help">{t(`${R}.operator.nodeIdHelp`)}</span>
                {fieldError("nodeId")}
              </label>
              <label className={`mg-field ${errors.p2pPort ? "has-error" : ""}`}>
                {t(`${R}.operator.p2pPort`)}
                <input data-testid="reg-p2p-port" inputMode="numeric" value={d.p2pPort} onChange={(e) => set({ p2pPort: e.target.value })} {...fieldProps("p2pPort")} />
                <span className="mg-field-help">{t(`${R}.operator.portDefault`)}</span>
                {fieldError("p2pPort")}
              </label>
              <label className={`mg-field ${errors.httpPort ? "has-error" : ""}`}>
                {t(`${R}.operator.httpPort`)}
                <input data-testid="reg-http-port" inputMode="numeric" value={d.httpPort} onChange={(e) => set({ httpPort: e.target.value })} {...fieldProps("httpPort")} />
                <span className="mg-field-help">{t(`${R}.operator.portDefault`)}</span>
                {fieldError("httpPort")}
              </label>
            </>
          )}
        </div>
      </>
    );
  } else if (step === "keys") {
    const k = keys.data;
    body = k ? (
      <>
        <div className="mg-field">
          {t(`${R}.keys.owner`)}
          <strong data-testid="reg-owner">{t(`${R}.keys.ownerValue`, { index: String(k.owner.index + 1) })}</strong>
          <span className="mg-field-help">{t(`${R}.keys.ownerHelp`)}</span>
        </div>
        <div className="mg-choices" role="radiogroup" aria-labelledby={`${id}-reg-voting`} data-testid="reg-voting">
          <span id={`${id}-reg-voting`} className="mg-section-title">{t(`${R}.keys.voting`)}</span>
          {choice("voting", "wallet", d.voting === "wallet", () => set({ voting: "wallet" }), t(`${R}.keys.votingWallet`), t(`${R}.keys.votingWalletText`, { index: String(k.voting.index + 1) }), "reg-voting-wallet")}
          {choice("voting", "address", d.voting === "address", () => set({ voting: "address" }), t(`${R}.keys.votingOther`), t(`${R}.keys.votingOtherText`), "reg-voting-other")}
        </div>
        {d.voting === "address" && (
          <label className={`mg-field mg-span ${errors.votingAddress ? "has-error" : ""}`}>
            {t(`${R}.keys.votingAddress`)}
            <input data-testid="reg-voting-address" className="mono" value={d.votingAddress} autoComplete="off" spellCheck={false} onChange={(e) => set({ votingAddress: e.target.value })} {...fieldProps("votingAddress")} />
            {fieldError("votingAddress")}
          </label>
        )}
        <div className="mg-choices" role="radiogroup" aria-labelledby={`${id}-reg-payout`} data-testid="reg-payout">
          <span id={`${id}-reg-payout`} className="mg-section-title">{t(`${R}.keys.payout`)}</span>
          {choice("payout", "new", d.payout === "new", () => set({ payout: "new" }), t(`${R}.keys.payoutNew`), t(`${R}.keys.payoutNewText`), "reg-payout-new")}
          {choice("payout", "address", d.payout === "address", () => set({ payout: "address" }), t(`${R}.keys.payoutOther`), t(`${R}.keys.payoutOtherText`), "reg-payout-other")}
        </div>
        {d.payout === "address" && (
          <label className={`mg-field mg-span ${errors.payoutAddress ? "has-error" : ""}`}>
            {t(`${R}.keys.payoutAddress`)}
            <input data-testid="reg-payout-address" className="mono" value={d.payoutAddress} autoComplete="off" spellCheck={false} onChange={(e) => set({ payoutAddress: e.target.value })} {...fieldProps("payoutAddress")} />
            {fieldError("payoutAddress")}
          </label>
        )}
      </>
    ) : keys.isPending ? (
      <LoadingState testId="reg-keys-loading" text={t(`${R}.review.checking`)} />
    ) : null;
  } else if (step === "review")
    body =
      preview.isPending || (preview.data && !figures) ? (
        <LoadingState testId="reg-preview-loading" text={t(`${R}.review.checking`)} />
      ) : figures ? (
        <Review d={d} p={figures} amount={amount} changed={changed} />
      ) : null;
  else if (step === "prove" && op)
    body = (
      <>
        <h4 className="mg-section-title">{t(`${R}.prove.heading`)}</h4>
        <p className="mg-small">{t(`${R}.prove.text`, { outpoint: op.terms.collateralOutpoint ? shortOutpoint(op.terms.collateralOutpoint) : "" })}</p>
        {op.signRequest && !needsNewMessage && (
        <dl className="detail-list card-l1" data-testid="reg-sign-request">
          <DetailRow label={t(`${R}.prove.address`)}>
            <span className="copy-value">
              <span className="mono" data-testid="reg-sign-address">
                {op.signRequest.address}
              </span>
              <CopyIconButton text={op.signRequest.address} label={t(`${R}.prove.address`)} />
            </span>
          </DetailRow>
          <DetailRow label={t(`${R}.prove.message`)}>
            <span className="copy-value">
              <span className="mono mg-break" data-testid="reg-sign-message">
                {op.signRequest.message}
              </span>
              <CopyIconButton text={op.signRequest.message} label={t(`${R}.prove.message`)} />
            </span>
          </DetailRow>
        </dl>
        )}
        <dl className="detail-list card-l1" data-testid="reg-prove-cost">
          <DetailRow label={t(`${R}.review.fee`)} testId="reg-prove-fee">
            {amount(op.fee)}
          </DetailRow>
          <DetailRow label={t(`${R}.review.total`)} testId="reg-prove-debit">
            {amount(op.debit)}
          </DetailRow>
        </dl>
        {needsNewMessage ? (
          <div className="mg-notice mg-notice-warning" role="alert" data-testid="reg-inputs-spent">
            <p className="mg-flush">{t(`${R}.refused.inputsSpent`)}</p>
            <DashButton variant="filledBlue" size="small" data-testid="reg-new-message" disabled={busy} onClick={() => void newMessage()}>
              {t(`${R}.prove.newMessage`)}
            </DashButton>
          </div>
        ) : (
          <label className={`mg-field mg-span ${errors.signature ? "has-error" : ""}`}>
            {t(`${R}.prove.signature`)}
            <textarea data-testid="reg-signature" className="mono" rows={3} value={signature} autoComplete="off" spellCheck={false} data-autofocus onChange={(e) => setSignature(e.target.value)} {...fieldProps("signature")} />
            <span className="mg-field-help">{t(`${R}.prove.signatureHelp`)}</span>
            {fieldError("signature")}
          </label>
        )}
        <p className="mg-small mg-muted">{t(`${R}.prove.note`)}</p>
        <p className="mg-small mg-muted" data-testid="reg-prove-saved">
          {t(`${R}.prove.saved`)}
        </p>
      </>
    );
  else if (step === "done" && op) body = <Done op={op} self={d.runner === "self"} />;

  const reviewing = step === "review";
  const proving = step === "prove";
  const action = finalAction(step, external, sends);
  const sendOff = action === "sendOff";
  const notSent = step === "done" && op?.state.kind === "not_sent";
  const title = d.kind === "evonode" || op?.terms.masternodeType === "evonode" ? t(`${R}.titleEvonode`) : t(`${R}.titleRegular`);
  const count = external || proving ? STEPS.length + 1 : STEPS.length;
  return (
    <Sheet title={title} width="large" testId="reg-sheet" closable={!busy} focusKey={step} onClose={close}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          if (sendOff || askDiscard) return;
          void (proving || notSent ? submitOrResend() : next());
        }}
      >
        <div className="mg-wizard">
          {step !== "done" && (
            <ol className="mg-steps" aria-label={t(`${R}.setUp`)}>
              {STEPS.map((s, i) => (
                <li key={s} data-n={i + 1} className={i < index || proving ? "is-done" : undefined} aria-current={s === step ? "step" : undefined}>
                  {t(`${R}.steps.${s}`)}
                </li>
              ))}
            </ol>
          )}
          <div className="mg-step" data-testid={`reg-step-${step}`}>
            <h3 className="mg-step-title">{stepHeading(step, d.kind, d.runner === "self", t)}</h3>
            {body}
            {sendOff ? (
              <p className="mg-notice mg-notice-warning" data-testid="reg-send-off">
                {t(`${R}.sendOff`)}
              </p>
            ) : (
              ((reviewing && !external) || proving || notSent) && <CredentialField c={credential} busy={busy} testId="reg" />
            )}
            <p className="form-problem" role="alert" data-testid="reg-general">
              {general}
            </p>
          </div>
        </div>
        {askDiscard ? (
          <footer className="mg-sheet-footer" data-testid="reg-discard">
            <span className="mg-sheet-footer-note">
              <strong>{t(`${R}.discardTitle`)}</strong> {t(`${R}.discardText`)}
            </span>
            <span className="mg-actions">
              <DashButton variant="tintedGray" size="medium" data-testid="reg-keep" onClick={() => setAskDiscard(false)}>
                {t(`${R}.keepEditing`)}
              </DashButton>
              <DashButton variant="plainRed" size="medium" data-testid="reg-discard-confirm" onClick={onClose}>
                {t(`${R}.discard`)}
              </DashButton>
            </span>
          </footer>
        ) : (
          <footer className="mg-sheet-footer mg-sheet-footer-sticky">
            <span className="mg-sheet-footer-note">
              {step !== "done" && t(`${R}.stepOf`, { step: String(proving ? count : index + 1), count: String(count) })}
              {reviewing && figures && <span className="mg-small mg-muted"> {d.collateral === "fund_new" ? t(`${R}.review.withCollateral`) : external ? t(`${R}.review.external`) : t(`${R}.review.onlyFee`)}</span>}
            </span>
            <span className="mg-actions">
              {step === "done" ? (
                <>
                  {op?.proTxHash && (
                    <DashButton variant="tintedBlue" size="medium" data-testid="reg-copy-hash" onClick={() => void copyWithToast(op.proTxHash!)}>
                      {t(`${R}.doneScreen.copyHash`)}
                    </DashButton>
                  )}
                  {notSent && (
                    <DashButton type="submit" variant="tintedBlue" size="medium" data-testid="reg-try-again" disabled={busy || !credential.ready}>
                      {t(`${R}.doneScreen.tryAgain`)}
                    </DashButton>
                  )}
                  <DashButton variant="filledBlue" size="medium" data-testid="reg-done" onClick={onClose}>
                    {t(`${R}.done`)}
                  </DashButton>
                </>
              ) : (
                <>
                  {step === "type" || (proving && resume) ? (
                    <DashButton variant="tintedGray" size="medium" data-testid="reg-cancel" disabled={busy} onClick={close}>
                      {t(`${R}.cancel`)}
                    </DashButton>
                  ) : (
                    <DashButton variant="tintedGray" size="medium" data-testid="reg-back" disabled={busy || proving} onClick={goBack}>
                      {t(`${R}.back`)}
                    </DashButton>
                  )}
                  {sendOff ? null : (
                    <DashButton
                      type="submit"
                      variant="filledBlue"
                      size="medium"
                      data-testid={action === "register" ? "reg-register" : action === "submit" ? "reg-submit" : "reg-continue"}
                      disabled={busy || (reviewing && !figures) || (action !== "continue" && !credential.ready) || (proving && needsNewMessage) || (step === "keys" && !keys.data)}
                    >
                      {busy && (reviewing || proving) ? t(`${R}.registering`) : action !== "continue" ? t(`${R}.register`) : t(`${R}.continue`)}
                    </DashButton>
                  )}
                </>
              )}
            </span>
          </footer>
        )}
      </form>
    </Sheet>
  );
}

const shortOutpoint = (o: string) => {
  const [tx, n] = o.split("-");
  return `${shortHash(tx)}:${n}`;
};

function stepHeading(step: Step, kind: MasternodeTypeName, self: boolean, t: ReturnType<typeof useT>): string {
  switch (step) {
    case "type":
      return t(`${R}.steps.type`);
    case "collateral":
      return t(`${R}.collateral.where`, { amount: collateralDash(kind) });
    case "operator":
      return t(self ? `${R}.operator.selfTitle` : `${R}.operator.hostedTitle`);
    case "keys":
      return t(`${R}.keys.title`);
    case "review":
      return t(`${R}.review.title`);
    case "prove":
      return t(`${R}.prove.title`);
    case "done":
      return t(`${R}.doneScreen.title`);
  }
}

function Review({ d, p, amount, changed }: { d: RegDraft; p: MnRegistrationPreview; amount: (duffs: number) => ReactNode; changed: boolean }) {
  const t = useT();
  const terms = p.terms;
  const self = d.runner === "self";
  const kind = t(d.kind === "evonode" ? "masternodes.myMasternodes.kind.evonode" : "masternodes.myMasternodes.kind.masternode");
  const outpoint = terms.collateralOutpoint ? shortOutpoint(terms.collateralOutpoint) : "";
  return (
    <>
      <dl className="detail-list card-l1" data-testid="reg-review">
        <DetailRow label={t(`${R}.review.type`)}>{t(`${R}.review.typeValue`, { kind, runner: t(self ? `${R}.review.runSelf` : `${R}.review.runHosted`) })}</DetailRow>
        <DetailRow label={t(`${R}.review.collateral`)} testId="reg-review-collateral">
          {d.collateral === "fund_new"
            ? t.rich(`${R}.review.collateralNew`, { amount: amount(p.collateralAmount) })
            : d.collateral === "existing"
              ? t.rich(`${R}.review.collateralExisting`, { outpoint: <span className="mono">{outpoint}</span>, amount: amount(p.collateralAmount) })
              : t.rich(`${R}.review.collateralExternal`, { outpoint: <span className="mono">{outpoint}</span> })}
          <span className="mg-small mg-muted" data-testid="reg-review-collateral-note"> {t(collateralNote(d.collateral))}</span>
        </DetailRow>
        <DetailRow label={t(`${R}.review.server`)}>
          <span className="mono">{terms.service}</span>
        </DetailRow>
        <DetailRow label={t(`${R}.review.operatorKey`)}>
          {terms.operatorIndex !== null
            ? t.rich(`${R}.review.fromWallet`, { key: <span className="mono">{shortHash(terms.operatorPublicKey)}</span>, index: String(terms.operatorIndex + 1) })
            : t.rich(`${R}.review.fromOperator`, { key: <span className="mono">{shortHash(terms.operatorPublicKey)}</span> })}
        </DetailRow>
        {terms.operatorReward > 0 && <DetailRow label={t(`${R}.review.share`)}>{t(`${R}.review.shareValue`, { share: terms.operatorReward / 100 })}</DetailRow>}
        {terms.platform && (
          <DetailRow label={t(`${R}.review.platformNode`)}>
            <span className="mono">
              {shortHash(terms.platform.nodeId)} · {terms.platform.p2pPort} / {terms.platform.httpPort}
            </span>
          </DetailRow>
        )}
        <DetailRow label={t(`${R}.review.voting`)}>{terms.votingIndex !== null ? t(`${R}.review.votingWallet`) : <span className="mono">{shortAddress(terms.votingAddress)}</span>}</DetailRow>
        <DetailRow label={t(`${R}.review.payout`)}>{d.payout === "new" ? t(`${R}.review.payoutNew`) : <span className="mono">{shortAddress(terms.payoutAddress)}</span>}</DetailRow>
        <DetailRow label={t(`${R}.review.fee`)} testId="reg-review-fee">
          {t.rich(`${R}.review.feeValue`, { fee: amount(p.fee) })}
        </DetailRow>
        <DetailRow label={t(`${R}.review.total`)} testId="reg-review-total">
          {amount(p.totalDebit)}
        </DetailRow>
        <DetailRow label={t(`${R}.review.then`)}>{t(self ? `${R}.review.thenSelf` : `${R}.review.thenHosted`)}</DetailRow>
      </dl>
      {changed && (
        <p className="mg-notice mg-notice-warning" role="alert" data-testid="reg-changed">
          {t(`${R}.review.changed`)}
        </p>
      )}
    </>
  );
}

function Done({ op, self }: { op: ProTxOperation; self: boolean }) {
  const t = useT();
  if (op.state.kind === "not_sent")
    return (
      <p className="mg-notice mg-notice-warning" role="alert" data-testid="reg-not-sent">
        {t(`${R}.doneScreen.notSent`, { reason: lastErrorText(op.lastError) ?? "" })}
      </p>
    );
  return (
    <div data-testid="reg-done-body">
      <p className="chip-row">
        <span className="chip chip-info">{t("masternodes.myMasternodes.chip.unconfirmed")}</span> <span className="mg-small mg-muted">{t(`${R}.doneScreen.waiting`)}</span>
      </p>
      {op.proTxHash && (
        <dl className="detail-list card-l1">
          <DetailRow label={t(`${R}.doneScreen.hash`)}>
            <span className="mono mg-hash" data-testid="reg-hash">
              {op.proTxHash.slice(0, 32)}
              <wbr />
              {op.proTxHash.slice(32)}
            </span>
          </DetailRow>
        </dl>
      )}
      <ol className="mg-small">
        <li>{t(self ? `${R}.doneScreen.stepSelf` : `${R}.doneScreen.stepHosted`)}</li>
        <li>{t(`${R}.doneScreen.stepActive`)}</li>
        <li>{t(`${R}.doneScreen.stepPayouts`)}</li>
      </ol>
    </div>
  );
}
