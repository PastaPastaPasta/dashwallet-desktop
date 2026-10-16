// The CoinJoin page (U-06), after Android's CoinJoin screens: the wallet's mixing state with Start
// and Stop Mixing, the mixing level (D2), what keeps mixing running (the lock, D5/D6), and dash-qt's
// panel and advanced view in a collapsed "Session details" (QT-042, D10). The numbers behind the
// level are on Settings ▸ CoinJoin.

import { useId, useState } from "react";
import type { CoinJoinStatus } from "../../api/generated/CoinJoinStatus";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useLockStatus } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton, Icon } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { CoinJoinText, LEVELS, levelOf, lockLine, poolMessageText, POOL_STATE_TEXT } from "../../coinjoin/model";
import { useCoinJoinAmount, useCoinJoinSettings, useCoinJoinView, useSaveCoinJoinSettings } from "../../coinjoin/queries";
import { useDisplay } from "../../format/display";
import { failureText } from "../../onboarding/model";
import { AUTO_LOCK_CHOICES, SecurityText } from "../../settings/security";
import { Page } from "../Routes";
import { useViewAction, ViewFacts } from "./CoinJoinCard";

const autoLockText = (secs: number) => AUTO_LOCK_CHOICES.find((c) => c.secs === secs)?.label ?? SecurityText.autoLockSeconds(secs);

export function CoinJoinPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const { view, status, retry } = useCoinJoinView(walletId, network);
  const act = useViewAction(walletId, network, retry);
  const lock = useLockStatus();
  const autoLockSecs = useUi((s) => s.prefs.autoLockSecs);
  const id = useId();
  const { action } = view;
  const lockText = lockLine(lock.data?.state);
  return (
    <Page route="coinJoin">
      <p className="coinjoin-subtitle">{CoinJoinText.subtitle}</p>
      <section className="coinjoin-status card-l1" data-testid="coinjoin-status" data-state={view.state} aria-labelledby={`${id}-title`}>
        <Icon id="coinjoin" size={56} className="coinjoin-status-icon" />
        <div className="coinjoin-card-body">
          <div role="status" className="coinjoin-card-live">
            <h2 className="coinjoin-status-title" id={`${id}-title`} data-testid="coinjoin-status-title">
              {view.state === "loading" ? "Loading…" : view.title}
              {view.busy && <span className="spinner coinjoin-spinner" aria-hidden="true" />}
            </h2>
            <ViewFacts view={view} testId="coinjoin-status" />
          </div>
          {act.problem && (
            <p className="form-problem" role="alert" data-testid="coinjoin-problem">
              {act.problem}
            </p>
          )}
        </div>
        {action && (
          <DashButton
            variant={action.kind === "stop" ? "tintedBlue" : "filledBlue"}
            className="coinjoin-status-action"
            data-testid="coinjoin-action"
            disabled={action.disabled || act.busy}
            onClick={() => act.press(action.kind)}
          >
            {action.label}
          </DashButton>
        )}
        {act.sheet}
      </section>
      <section className="coinjoin-notes card-l1" data-testid="coinjoin-notes" aria-label="While mixing">
        <p>{CoinJoinText.keepOpen}</p>
        {lockText && <p data-testid="coinjoin-lock-line">{lockText}</p>}
        {autoLockSecs !== null && <p data-testid="coinjoin-auto-lock">{CoinJoinText.autoLock(autoLockText(autoLockSecs))}</p>}
      </section>
      <LevelCard mixing={status?.state === "mixing"} />
      {status && <SessionDetails status={status} network={network} />}
      <DashButton variant="plainBlue" data-testid="coinjoin-open-settings" onClick={() => navigate("settings", { settingsTab: "coinjoin" })}>
        CoinJoin settings
      </DashButton>
    </Page>
  );
}

/** Intermediate or Advanced (D2): writes the network's `rounds`; any other value shows as Custom,
 * set on Settings ▸ CoinJoin. */
function LevelCard({ mixing }: { mixing: boolean }) {
  const settings = useCoinJoinSettings();
  const save = useSaveCoinJoinSettings();
  /** The level as clicked, until the engine has answered (U-04 D36). */
  const [chosen, setChosen] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const id = useId();
  const current = settings.data;
  const rounds = chosen ?? current?.rounds;
  const pick = async (n: number) => {
    if (!current || chosen !== null) return;
    setChosen(n);
    setProblem(null);
    try {
      await save({ ...current, rounds: n });
    } catch (e) {
      setProblem(failureText(e));
    } finally {
      setChosen(null);
    }
  };
  return (
    <section className="coinjoin-levels card-l1" data-testid="coinjoin-levels" aria-labelledby={`${id}-title`}>
      <h2 className="card-title" id={`${id}-title`}>
        {CoinJoinText.levelTitle}
      </h2>
      <p className="field-hint">{CoinJoinText.levelSubtitle}</p>
      {settings.isError && (
        <p className="form-problem" role="alert" data-testid="coinjoin-levels-error">
          {failureText(settings.error)}
        </p>
      )}
      {current && (
        <fieldset className="radio-group coinjoin-level-group">
          <legend className="sr-only">{CoinJoinText.levelTitle}</legend>
          {LEVELS.map((l) => (
            <label key={l.id} className="coinjoin-level" data-testid={`coinjoin-level-${l.id}`}>
              <input
                type="radio"
                name={`${id}-level`}
                checked={rounds === l.rounds}
                disabled={chosen !== null}
                onChange={() => void pick(l.rounds)}
              />
              <Icon id={l.id === "intermediate" ? "coinjoinShuffle" : "coinjoinProtected"} size={24} className="coinjoin-level-icon" />
              <span className="coinjoin-level-text">
                <span className="coinjoin-level-title">{l.title}</span>
                <span className="coinjoin-level-time">
                  {CoinJoinText.roundsOf(l.rounds)} · {l.time}
                </span>
                <span className="field-hint">{l.description}</span>
              </span>
            </label>
          ))}
          {rounds !== undefined && levelOf(rounds) === "custom" && (
            <label className="coinjoin-level" data-testid="coinjoin-level-custom">
              <input type="radio" name={`${id}-level`} checked readOnly disabled />
              <span className="coinjoin-level-text">
                <span className="coinjoin-level-title">{CoinJoinText.custom(rounds)}</span>
                <span className="field-hint">{CoinJoinText.customHint}</span>
              </span>
            </label>
          )}
        </fieldset>
      )}
      {mixing && <p className="field-hint">{CoinJoinText.levelLive}</p>}
      {problem && (
        <p className="form-problem" role="alert" data-testid="coinjoin-level-problem">
          {problem}
        </p>
      )}
    </section>
  );
}

/** dash-qt's panel, its progress tooltip and the advanced view's session table (QT-042, QT-047). */
function SessionDetails({ status, network }: { status: CoinJoinStatus; network: NetworkInfo }) {
  const amount = useCoinJoinAmount(network);
  const discreet = useUi((s) => s.discreet);
  const display = useDisplay(network);
  const shown = (duffs: number) => (discreet ? `${display.masked()} ${display.name}` : amount(duffs));
  const { progress, amountAndRounds: ar } = status;
  const pct = (n: number) => `${Math.floor(n)}%`;
  return (
    <details className="disclosure card-l1 coinjoin-details" data-testid="coinjoin-details">
      <summary>{CoinJoinText.details}</summary>
      <dl className="coinjoin-details-list">
        <DetailRow label={CoinJoinText.overallProgress} testId="coinjoin-detail-progress">
          {pct(progress.overallPercent)}
          <span className="field-hint coinjoin-detail-note">
            {CoinJoinText.denominated} {pct(progress.denominatedPercent)} · {CoinJoinText.partiallyMixed} {pct(progress.partiallyMixedPercent)} ·{" "}
            {CoinJoinText.mixed} {pct(progress.mixedPercent)}
          </span>
          <span className="field-hint coinjoin-detail-note">{CoinJoinText.averageRounds(progress.averageRounds.toFixed(2), ar.rounds)}</span>
        </DetailRow>
        <DetailRow label={CoinJoinText.amountAndRounds} testId="coinjoin-detail-amount">
          {shown(ar.amount)} / {CoinJoinText.roundsOf(ar.rounds)}
          {ar.insufficientInputs && <span className="field-hint coinjoin-detail-note">{CoinJoinText.notEnoughInputs}</span>}
        </DetailRow>
        <DetailRow label={CoinJoinText.submittedDenominations} testId="coinjoin-detail-denoms">
          {status.submittedDenominations.length === 0 ? "—" : status.submittedDenominations.map(shown).join(", ")}
        </DetailRow>
        <DetailRow label={CoinJoinText.queueSize} testId="coinjoin-detail-queue">
          {status.queueSize}
        </DetailRow>
      </dl>
      <div className="table-card coinjoin-sessions" data-testid="coinjoin-sessions">
        {status.sessions.length === 0 ? (
          <p className="field-hint">{CoinJoinText.noSessions}</p>
        ) : (
          <table className="data-table">
            <caption className="sr-only">{CoinJoinText.sessions}</caption>
            <thead>
              <tr>
                <th scope="col">Masternode</th>
                <th scope="col">Denomination</th>
                <th scope="col">State</th>
                <th scope="col" className="num">
                  Entries
                </th>
                <th scope="col">Last message</th>
              </tr>
            </thead>
            <tbody>
              {status.sessions.map((s, i) => (
                <tr key={s.proTxHash ?? i}>
                  <td className="mono">{s.service ?? "—"}</td>
                  <td>{s.denomination === null ? "—" : shown(s.denomination)}</td>
                  <td>{POOL_STATE_TEXT[s.state]}</td>
                  <td className="num">{s.entries}</td>
                  <td>{s.lastMessage === null ? "—" : poolMessageText(s.lastMessage)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </details>
  );
}
