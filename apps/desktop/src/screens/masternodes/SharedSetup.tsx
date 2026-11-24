// The coordinator's setup (MG-10 §2.2, `sh-setup`, `sh-terms`): Shares · Masternode · Exit terms ·
// Your coins · Invite. Each step fills one part of `StartSharedRequest` and is checked before Continue,
// mirroring MG-04b §5; nothing reaches the engine before [Create invitation], which reserves coins and
// signs nothing (no grant). The Invite step shows the message to copy or save, then opens the session.

import { useId, useState, type ReactNode } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { OperatorChoice } from "../../api/generated/OperatorChoice";
import type { RewardChoice } from "../../api/generated/RewardChoice";
import type { SharedOutcome } from "../../api/generated/SharedOutcome";
import type { StartSharedRequest } from "../../api/generated/StartSharedRequest";
import type { VotingChoice } from "../../api/generated/VotingChoice";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend, useBalance } from "../../api/queries";
import { saveText } from "../../app/files";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { useT } from "../../i18n";
import { useMasternodesRefresh } from "../../masternodes/queries";
import {
  COIN,
  DEFAULT_FEE,
  isIpv4Service,
  MAX_SHARES,
  operatorKeyProblem,
  parseDash,
  parsePercent,
  penaltyError,
  PERIOD_PRESETS,
  reservationNote,
  SHARE_TOTAL,
  shareErrors,
  sharedErrorText,
  type SetupShare,
} from "../../masternodes/shared";
import { useSharedText } from "./Shared";

const P = "masternodes.shared.setup";
const STEPS = ["shares", "masternode", "exit", "coins", "invite"] as const;
type Step = (typeof STEPS)[number];

interface Row {
  label: string;
  amount: string;
}

type Operator = "given" | "self";
type Voting = "wallet" | "address";
type Reward = RewardChoice["kind"];

export function SharedSetup({ wallet, network, onClose, onCreated }: { wallet: WalletSummary; network: NetworkInfo; onClose: () => void; onCreated: (id: string) => void }) {
  const t = useT();
  const b = useBackend();
  const refresh = useMasternodesRefresh();
  const { dash, networkName } = useSharedText(network);
  const balance = useBalance(wallet.walletId);
  const id = useId();
  const [step, setStep] = useState<Step>("shares");
  const [rows, setRows] = useState<Row[]>([
    { label: wallet.name, amount: "500" },
    { label: "", amount: "500" },
  ]);
  const [service, setService] = useState("");
  const [operator, setOperator] = useState<Operator>("given");
  const [operatorKey, setOperatorPublic] = useState("");
  const [operatorShare, setOperatorShare] = useState("0");
  const [voting, setVoting] = useState<Voting>("wallet");
  const [votingAddress, setVotingAddress] = useState("");
  const [runBy, setRunBy] = useState("");
  const [period, setPeriod] = useState<number>(PERIOD_PRESETS[2].blocks);
  const [penalty, setPenalty] = useState("0");
  const [reward, setReward] = useState<Reward>("new_address");
  const [rewardAddress, setRewardAddress] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showProblems, setShowProblems] = useState(false);
  const [created, setCreated] = useState<SharedOutcome | null>(null);

  const whole = (duffs: number) => new Intl.NumberFormat(t.language.tag, { maximumFractionDigits: 8 }).format(duffs / COIN);
  const shares: SetupShare[] = rows.map((r) => ({ label: r.label, amount: parseDash(r.amount) ?? 0 }));
  const sum = shares.reduce((a, s) => a + s.amount, 0);
  const smallest = shares.length ? Math.min(...shares.map((s) => s.amount)) : 0;
  const penaltyDuffs = period === 0 ? 0 : parseDash(penalty);
  const operatorBp = parsePercent(operatorShare);
  const keyProblem = operatorKeyProblem(operatorKey);

  const problems: Record<Step, string[]> = {
    shares: shareErrors(shares).map((e) => t(`${P}.error.${e}`)),
    masternode: [
      ...(isIpv4Service(service) ? [] : [t(`${P}.error.service`)]),
      ...(operator === "given" && keyProblem ? [t(keyProblem === "secret" ? `${P}.error.operator_secret` : `${P}.error.operator_invalid`)] : []),
      ...(operatorBp === null ? [t(`${P}.error.operatorShare`)] : []),
      ...(voting === "address" && votingAddress.trim() === "" ? [t(`${P}.error.address`)] : []),
    ],
    exit: penaltyDuffs === null ? [t(`${P}.error.penaltyFormat`)] : [penaltyError(shares, period, penaltyDuffs)].filter((e): e is "penalty" | "penaltyNoPeriod" => !!e).map((e) => t(`${P}.error.${e}`)),
    coins: reward === "address" && rewardAddress.trim() === "" ? [t(`${P}.error.address`)] : [],
    invite: [],
  };

  const request = (): StartSharedRequest => {
    const op: OperatorChoice = operator === "self" ? { kind: "next_unused" } : { kind: "given", publicKey: operatorKey.trim() };
    const vote: VotingChoice = voting === "wallet" ? { kind: "next_unused" } : { kind: "address", address: votingAddress.trim() };
    const rewardChoice: RewardChoice = reward === "address" ? { kind: "address", address: rewardAddress.trim() } : { kind: reward };
    return {
      shares: shares.map((s) => ({ label: s.label.trim(), amount: s.amount })),
      terms: { service: service.trim(), operator: op, operatorReward: operatorBp ?? 0, voting: vote, earlyPeriodBlocks: period, earlyPenalty: penaltyDuffs ?? 0 },
      fee: DEFAULT_FEE,
      reward: rewardChoice,
      nodeRunBy: runBy.trim() || null,
    };
  };

  const index = STEPS.indexOf(step);
  const next = async () => {
    if (problems[step].length) return setShowProblems(true);
    setShowProblems(false);
    setError(null);
    if (step !== "coins") return setStep(STEPS[index + 1]);
    setBusy(true);
    try {
      setCreated(await b.mnSharedStart(wallet.walletId, request()));
      setStep("invite");
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, dash }));
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  const copy = async () => {
    if (!created) return;
    try {
      await navigator.clipboard.writeText(created.message.text);
      showToast(t("masternodes.shared.session.copied"));
    } catch {
      setError(t("masternodes.shared.session.copyFailed"));
    }
  };
  const save = async () => {
    if (!created) return;
    try {
      await saveText(b, "sharedMasternodeMessage", created.message.fileName, created.message.text);
    } catch (e) {
      setError(sharedErrorText(e, { network: networkName, dash }));
    }
  };

  const field = (key: string, label: string, input: ReactNode, help?: string) => (
    <div className="form-field" key={key}>
      <label htmlFor={`${id}-${key}`}>{label}</label>
      {input}
      {help && <span className="mg-field-help">{help}</span>}
    </div>
  );

  const body: Record<Step, ReactNode> = {
    shares: (
      <>
        <div className="mg-choices" role="group" aria-label={t(`${P}.typeHeading`)}>
          <label className="mg-choice" data-testid="sh-setup-type-regular">
            <input type="radio" name={`${id}-type`} checked readOnly />
            <span className="mg-choice-text">
              <span>{t(`${P}.typeMasternode`)}</span>
              <span className="mg-muted mg-small">{t(`${P}.typeMasternodeText`)}</span>
            </span>
          </label>
          <label className="mg-choice is-disabled" data-testid="sh-setup-type-evonode">
            <input type="radio" name={`${id}-type`} disabled />
            <span className="mg-choice-text">
              <span>{t(`${P}.typeEvonode`)}</span>
              <span className="mg-muted mg-small">{t("masternodes.shared.evonodeOff")}</span>
            </span>
          </label>
        </div>
        <ol className="sh-setup-rows" data-testid="sh-setup-rows">
          {rows.map((r, i) => (
            <li key={i} className="sh-setup-row" data-testid={`sh-setup-row-${i}`}>
              <div className="form-field">
                <label htmlFor={`${id}-label-${i}`}>{i === 0 ? t(`${P}.you`, { name: t(`${P}.label`) }) : t(`${P}.label`)}</label>
                <input id={`${id}-label-${i}`} data-testid={`sh-setup-label-${i}`} value={r.label} maxLength={64} onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, label: e.target.value } : x)))} />
              </div>
              <div className="form-field">
                <label htmlFor={`${id}-amount-${i}`}>{t(`${P}.amount`)}</label>
                <input id={`${id}-amount-${i}`} inputMode="decimal" data-testid={`sh-setup-amount-${i}`} value={r.amount} onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, amount: e.target.value } : x)))} />
              </div>
              {i > 0 && rows.length > 2 && (
                <DashButton variant="plainRed" size="small" data-testid={`sh-setup-remove-${i}`} onClick={() => setRows(rows.filter((_, j) => j !== i))}>
                  {t(`${P}.remove`)}
                </DashButton>
              )}
            </li>
          ))}
        </ol>
        {rows.length < MAX_SHARES && (
          <DashButton variant="tintedBlue" size="small" data-testid="sh-setup-add" onClick={() => setRows([...rows, { label: "", amount: "100" }])}>
            {t(`${P}.add`)}
          </DashButton>
        )}
        <p className="mg-small" data-testid="sh-setup-total" aria-live="polite">
          {t(`${P}.total`, { sum: whole(sum), count: String(rows.length) })}
          {sum !== SHARE_TOTAL && ` · ${t(sum < SHARE_TOTAL ? `${P}.unassigned` : `${P}.tooMuch`, { amount: whole(Math.abs(SHARE_TOTAL - sum)) })}`}
        </p>
        <p className="mg-muted mg-small">{t(`${P}.sharesHelp`)}</p>
      </>
    ),
    masternode: (
      <>
        {field("service", t(`${P}.service`), <input id={`${id}-service`} data-testid="sh-setup-service" placeholder="203.0.113.7:19999" value={service} onChange={(e) => setService(e.target.value)} />, t(`${P}.serviceHelp`))}
        <fieldset className="mg-choices" data-testid="sh-setup-operator">
          <legend className="mg-section-title">{t(`${P}.operator`)}</legend>
          <label className="mg-choice">
            <input type="radio" name={`${id}-operator`} checked={operator === "given"} onChange={() => setOperator("given")} />
            <span className="mg-choice-text">{t(`${P}.operatorGiven`)}</span>
          </label>
          <label className="mg-choice">
            <input type="radio" name={`${id}-operator`} data-testid="sh-setup-operator-self" checked={operator === "self"} onChange={() => setOperator("self")} />
            <span className="mg-choice-text">
              <span>{t(`${P}.operatorSelf`)}</span>
              <span className="mg-muted mg-small">{t(`${P}.operatorSelfText`)}</span>
            </span>
          </label>
        </fieldset>
        {operator === "given" &&
          field("operatorKey", t(`${P}.operatorKey`), <input id={`${id}-operatorKey`} className="mono" data-testid="sh-setup-operator-key" spellCheck={false} autoComplete="off" value={operatorKey} onChange={(e) => setOperatorPublic(e.target.value)} />, t(`${P}.operatorKeyHelp`))}
        {field("operatorShare", t(`${P}.operatorShare`), <input id={`${id}-operatorShare`} inputMode="decimal" data-testid="sh-setup-operator-share" value={operatorShare} onChange={(e) => setOperatorShare(e.target.value)} />, t(`${P}.operatorShareHelp`))}
        <fieldset className="mg-choices" data-testid="sh-setup-voting">
          <legend className="mg-section-title">{t(`${P}.votes`)}</legend>
          <label className="mg-choice">
            <input type="radio" name={`${id}-voting`} checked={voting === "wallet"} onChange={() => setVoting("wallet")} />
            <span className="mg-choice-text">{t(`${P}.votesWallet`)}</span>
          </label>
          <label className="mg-choice">
            <input type="radio" name={`${id}-voting`} data-testid="sh-setup-voting-address" checked={voting === "address"} onChange={() => setVoting("address")} />
            <span className="mg-choice-text">{t(`${P}.votesAddress`)}</span>
          </label>
        </fieldset>
        {voting === "address" && field("votingAddress", t(`${P}.votesAddressLabel`), <input id={`${id}-votingAddress`} data-testid="sh-setup-voting-address-input" spellCheck={false} value={votingAddress} onChange={(e) => setVotingAddress(e.target.value)} />)}
        {field("runBy", t("masternodes.shared.session.runBy"), <input id={`${id}-runBy`} data-testid="sh-setup-run-by" maxLength={64} value={runBy} onChange={(e) => setRunBy(e.target.value)} />, t(`${P}.runByHelp`))}
      </>
    ),
    exit: (
      <>
        <p className="mg-muted">{t(`${P}.exitText`)}</p>
        {field(
          "period",
          t(`${P}.period`),
          <select id={`${id}-period`} data-testid="sh-setup-period" value={period} onChange={(e) => setPeriod(Number(e.target.value))}>
            {PERIOD_PRESETS.map((p) => (
              <option key={p.blocks} value={p.blocks}>
                {t(`${P}.periods.${p.key}`)}
              </option>
            ))}
          </select>,
          t(`${P}.periodHelp`),
        )}
        {field(
          "penalty",
          t(`${P}.penalty`),
          <input id={`${id}-penalty`} inputMode="decimal" data-testid="sh-setup-penalty" disabled={period === 0} value={period === 0 ? "0" : penalty} onChange={(e) => setPenalty(e.target.value)} />,
          t(`${P}.penaltyHelp`, { smallest: whole(smallest) }),
        )}
        {period > 0 && penaltyDuffs === 0 && (
          <p className="mg-notice" data-testid="sh-setup-zero-penalty">
            {t("masternodes.shared.warning.zero_penalty")}
          </p>
        )}
      </>
    ),
    coins: (
      <>
        <p data-testid="sh-setup-estimate">
          {balance.data
            ? t("masternodes.shared.funding.estimateCoordinator", { amount: dash((shares[0]?.amount ?? 0) + DEFAULT_FEE), balance: dash(balance.data.confirmed), share: dash(shares[0]?.amount ?? 0), fee: dash(DEFAULT_FEE) })
            : t("masternodes.shared.funding.estimate", { amount: dash((shares[0]?.amount ?? 0) + DEFAULT_FEE) })}
        </p>
        <p className="mg-muted mg-small">{t("masternodes.shared.funding.feeYours", { fee: dash(DEFAULT_FEE) })}</p>
        <fieldset className="mg-choices" data-testid="sh-setup-reward">
          <legend className="mg-section-title">{t(`${P}.reward`)}</legend>
          {(["new_address", "refund", "address"] as const).map((k) => (
            <label className="mg-choice" key={k}>
              <input type="radio" name={`${id}-reward`} data-testid={`sh-setup-reward-${k}`} checked={reward === k} onChange={() => setReward(k)} />
              <span className="mg-choice-text">{t(`${P}.${k === "new_address" ? "rewardNew" : k === "refund" ? "rewardRefund" : "rewardOther"}`)}</span>
            </label>
          ))}
        </fieldset>
        {reward === "address" && field("rewardAddress", t(`${P}.rewardAddressLabel`), <input id={`${id}-rewardAddress`} data-testid="sh-setup-reward-address" spellCheck={false} value={rewardAddress} onChange={(e) => setRewardAddress(e.target.value)} />)}
        <p className="mg-muted mg-small">{t(`${P}.nothingSent`)}</p>
        <p className="mg-muted mg-small" data-testid="sh-reservation-note">
          {reservationNote(network)}
        </p>
      </>
    ),
    invite: created && (
      <>
        <p>{t(`${P}.inviteText`)}</p>
        <dl className="mg-facts" data-testid="sh-setup-codes">
          <dt>{t(`${P}.sessionCode`)}</dt>
          <dd className="mono">{created.session.code}</dd>
          <dt>{t(`${P}.checkCode`)}</dt>
          <dd className="mono" data-testid="sh-setup-check-code">
            {created.message.code}
          </dd>
        </dl>
        <p className="mg-muted mg-small">{t(`${P}.readBack`)}</p>
        <span className="mg-actions">
          <DashButton variant="filledBlue" size="small" data-testid="sh-setup-copy" onClick={() => void copy()}>
            {t("masternodes.shared.row.copyInvitation")}
          </DashButton>
          <DashButton variant="tintedBlue" size="small" data-testid="sh-setup-save" onClick={() => void save()}>
            {t("masternodes.shared.session.saveFile")}
          </DashButton>
        </span>
      </>
    ),
  };

  return (
    <Sheet title={t(`${P}.title`)} width="large" testId="sh-setup" closable={!busy} focusKey={step} onClose={created ? () => onCreated(created.session.id) : onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          if (step === "invite" && created) onCreated(created.session.id);
          else void next();
        }}
      >
        <div className="mg-wizard">
          <ol className="mg-steps" aria-label={t("masternodes.shared.stages.label")}>
            {STEPS.map((s, i) => (
              <li key={s} data-n={i + 1} className={i < index ? "is-done" : undefined} aria-current={s === step ? "step" : undefined}>
                {t(`${P}.steps.${s}`)}
              </li>
            ))}
          </ol>
          <div className="mg-step" data-testid={`sh-setup-step-${step}`}>
            <h3 className="mg-step-title">{t(`${P}.headings.${step}`)}</h3>
            {body[step]}
            <div className="form-problem" role="alert" data-testid="sh-setup-problems">
              {showProblems && problems[step].map((p) => <p key={p}>{p}</p>)}
              {error && <p>{error}</p>}
            </div>
          </div>
        </div>
        <footer className="mg-sheet-footer">
          <span className="mg-sheet-footer-note">{t(`${P}.stepOf`, { step: String(index + 1), total: String(STEPS.length) })}</span>
          <span className="mg-actions">
            {step === "invite" ? null : index > 0 ? (
              <DashButton variant="tintedGray" size="medium" data-testid="sh-setup-back" disabled={busy} onClick={() => setStep(STEPS[index - 1])}>
                {t(`${P}.back`)}
              </DashButton>
            ) : (
              <DashButton variant="tintedGray" size="medium" data-testid="sh-setup-cancel" onClick={onClose}>
                {t(`${P}.cancel`)}
              </DashButton>
            )}
            <DashButton type="submit" variant="filledBlue" size="medium" data-testid="sh-setup-continue" disabled={busy}>
              {step === "coins" ? t(`${P}.create`) : step === "invite" ? t(`${P}.done`) : t(`${P}.next`)}
            </DashButton>
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
