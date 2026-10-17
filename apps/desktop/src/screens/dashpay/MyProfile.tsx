// My Profile (DASHPAY §4 `MyProfileViewModel`, F5, F7, F8; DP1-10 P1–P6): the header, usernames,
// identity id, the credits block with its warnings and its pending or stuck top-ups, the own
// contest's card while one runs, and the contest outcome rows. Without a main identity it is
// DP1-08's card for the state (D2). The view models are dashpay/profile.ts and dashpay/topup.ts;
// the Settings card (D20) is here too.

import { useId, useState } from "react";
import type { Backend } from "../../api/backend";
import type { IdentitySummary } from "../../api/generated/IdentitySummary";
import type { PendingTopUp } from "../../api/generated/PendingTopUp";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { mainIdentityOf, useBackend, useDashPayShell, useWallets } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { creditsModel, headerModel, holdsTopUp, outcomeRows, pageModel, PROFILE_COPY, requestModel, type ContestRef, type CreditsModel } from "../../dashpay/profile";
import { useAvatar, useContest, useCostTable, useOutcomeEvents, usePendingTopUps, useRefreshBalanceOnVisit, useStuckTopUps } from "../../dashpay/profileQueries";
import { useStatusInputs } from "../../dashpay/queries";
import type { CardActionKind, StatusInputs } from "../../dashpay/status";
import { outcomeText, topUpErrorText, topUpFor, type TopUpFlow } from "../../dashpay/topup";
import { wizardFor } from "../../dashpay/wizard";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { Page } from "../Routes";
import { SettingsCard } from "../settings/parts";
import { VotingInfoSheet } from "./AboutDashPay";
import { DashPayCard, defaultCardAction } from "./DashPayCard";
import { TopUpSheet } from "./TopUpSheet";

/** Opens DP1-09's credits path for `identity` on the Join page; `instant` refuses a contested
 * label (D13). */
function chooseName(b: Backend, walletId: string, identity: string, instant: boolean) {
  wizardFor(b, walletId).begin({ kind: "credits", identity, instant });
  navigate("joinDashPay");
}

/** Home's "Buy credits" (S19, S20): My Profile, with the top-up sheet open unless a pending or
 * stuck top-up of the identity holds Top up there (D8); a failed read opens it (the engine refuses). */
export function buyCredits(b: Backend, walletId: string, main: IdentitySummary) {
  navigate("profile");
  const open = () => topUpFor(b, walletId).begin(main.identity);
  void Promise.all([b.creditsPendingTopUps(walletId), b.creditsStuckTopUpFundings(walletId)]).then(([pending, stuck]) => {
    if (!holdsTopUp(main, pending, stuck)) open();
  }, open);
}

function Header({ main, contest, walletId }: { main: IdentitySummary; contest: string | null; walletId: string }) {
  const b = useBackend();
  const h = headerModel(main, contest);
  const png = useAvatar(walletId, main.identity, "large");
  return (
    <section className="profile-header card-l1" data-testid="profile-header">
      <Avatar letters={h.initials} size="pane" png={png} />
      <div className="profile-header-text">
        <h2 className="profile-name" data-testid="profile-name">
          {h.title}
        </h2>
        <p className="profile-username" data-testid="profile-username" data-names={h.names}>
          {h.username}
        </p>
        {h.message && <p className="profile-message">{h.message}</p>}
        {h.names === "none" && contest === null && (
          <DashButton variant="tintedBlue" size="small" data-testid="profile-choose-name" onClick={() => chooseName(b, walletId, main.identity, false)}>
            {PROFILE_COPY.chooseName}
          </DashButton>
        )}
      </div>
      <div className="profile-edit">
        <DashButton variant="tintedGray" size="small" disabled aria-describedby="profile-edit-later" data-testid="profile-edit">
          {PROFILE_COPY.editProfile}
        </DashButton>
        <span className="field-hint" id="profile-edit-later">
          {PROFILE_COPY.editLater}
        </span>
      </div>
    </section>
  );
}

function Details({ main, contest }: { main: IdentitySummary; contest: string | null }) {
  return (
    <section className="card-l1 profile-details" data-testid="profile-details" aria-label={PROFILE_COPY.identity}>
      <dl className="detail-list">
        <DetailRow label={PROFILE_COPY.usernames} testId="profile-usernames">
          {main.names.length > 0 ? main.names.map((n) => `@${n}`).join(", ") : headerModel(main, contest).username}
        </DetailRow>
        <DetailRow label={PROFILE_COPY.identity} testId="profile-identity">
          <CopyValue value={main.identity} label="identity id" mono />
        </DetailRow>
      </dl>
    </section>
  );
}

function Credits({ model, flow, rows, inputs, onTopUp }: { model: CreditsModel; flow: TopUpFlow; rows: Map<string, PendingTopUp>; inputs: StatusInputs; onTopUp: () => void }) {
  const id = useId();
  const C = PROFILE_COPY.credits;
  const result = flow.rowResult;
  return (
    <section className="card-l1 profile-credits" data-testid="profile-credits" data-level={model.level} aria-labelledby={`${id}-title`}>
      <h2 className="card-title" id={`${id}-title`}>
        {model.figure === null ? C.unknown : C.title}
      </h2>
      {model.figure === null ? (
        <p className="profile-credits-line">{C.unknownLine}</p>
      ) : (
        <>
          <p className="profile-credits-figure" data-testid="profile-credits-figure">
            {model.figure}
            {model.updating && <span className="profile-credits-updating">{C.updating}</span>}
          </p>
          {model.estimate && (
            <p className="profile-credits-line" data-testid="profile-credits-estimate">
              {model.estimate}
            </p>
          )}
        </>
      )}
      {model.warning && (
        <div className="profile-warning" role="status" data-testid="profile-credits-warning">
          <p className="profile-warning-title">{model.warning.title}</p>
          <p className="profile-credits-line">{model.warning.text}</p>
        </div>
      )}
      {model.pending.map((p) => (
        <div key={p.txid} className="profile-row" data-testid={`profile-pending-${p.txid}`}>
          <div>
            <p className="profile-row-title">{p.title}</p>
            <p className="profile-credits-line">{p.line}</p>
            {result?.txid === p.txid && (
              <p className={result.error ? "form-problem" : "form-ok"} role={result.error ? "alert" : "status"} data-testid="profile-pending-result">
                {result.error ? topUpErrorText(result.error, inputs.amount, inputs.discreet) : result.outcome && outcomeText(result.outcome, () => "").title}
              </p>
            )}
          </div>
          <div className="profile-row-actions">
            {p.arrived && (
              <DashButton variant="tintedGray" size="small" data-testid="profile-pending-arrived" disabled={flow.rowRunning !== null} onClick={() => void flow.creditsArrived(rows.get(p.txid)!)}>
                {PROFILE_COPY.pending.arrived}
              </DashButton>
            )}
            <DashButton variant="tintedBlue" size="small" data-testid="profile-pending-finish" disabled={flow.rowRunning !== null} onClick={() => void flow.finish(rows.get(p.txid)!)}>
              {PROFILE_COPY.pending.finish}
            </DashButton>
          </div>
        </div>
      ))}
      {model.stuck.map((s) => (
        <div key={s.step} className="profile-row" data-testid="profile-stuck">
          <div>
            <p className="profile-row-title">{s.title}</p>
            <p className="profile-credits-line">{s.text}</p>
          </div>
          <div className="profile-row-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="profile-stuck-repair" onClick={() => navigate("tools", { toolsTab: "repair" })}>
              {PROFILE_COPY.stuck.action}
            </DashButton>
          </div>
        </div>
      ))}
      {model.unseen && (
        <div className="profile-row" data-testid="profile-top-up-unseen">
          <p className="profile-row-title" role="status">
            {C.unseen}
          </p>
          <div className="profile-row-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="profile-top-up-show" onClick={onTopUp}>
              {C.showResult}
            </DashButton>
          </div>
        </div>
      )}
      <div className="profile-credits-actions">
        <DashButton
          variant={model.topUp.primary ? "filledBlue" : "tintedBlue"}
          data-testid="profile-top-up"
          disabled={model.topUp.disabled}
          aria-describedby={model.topUp.reason ? `${id}-reason` : undefined}
          onClick={onTopUp}
        >
          {C.topUp}
        </DashButton>
        {model.topUp.reason && (
          <span className="field-hint" id={`${id}-reason`} data-testid="profile-top-up-reason">
            {model.topUp.reason}
          </span>
        )}
      </div>
    </section>
  );
}

function RequestCard({ walletId, main, contest, inputs }: { walletId: string; main: IdentitySummary; contest: ContestRef; inputs: StatusInputs }) {
  const b = useBackend();
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  const read = useContest(walletId, main.identity, contest.label);
  const m = requestModel(contest.label, contest.endsAt, read.data, read.isError, main, inputs.date);
  return (
    <section className="card-l1 profile-request" data-testid="profile-request" data-state={m.state} aria-labelledby={`${id}-title`}>
      <h2 className="card-title" id={`${id}-title`}>
        {m.title}
      </h2>
      {m.lines.map((line) => (
        <p key={line} className="profile-credits-line">
          {line}
        </p>
      ))}
      {m.tally && (
        <dl className="detail-list profile-tally" data-testid="profile-tally">
          {m.tally.map((t) => (
            <DetailRow key={t.label} label={t.label}>
              {t.value}
            </DetailRow>
          ))}
        </dl>
      )}
      {m.readFailed && (
        <p className="form-problem" role="alert" data-testid="profile-request-failed">
          {PROFILE_COPY.request.readFailed}{" "}
          <DashButton variant="plainBlue" size="small" data-testid="profile-request-retry" onClick={() => void read.refetch()}>
            {PROFILE_COPY.request.retry}
          </DashButton>
        </p>
      )}
      <div className="profile-request-actions">
        {m.instant && (
          <>
            <DashButton variant="tintedBlue" size="small" data-testid="profile-instant" onClick={() => chooseName(b, walletId, main.identity, true)}>
              {PROFILE_COPY.request.instant}
            </DashButton>
            <span className="field-hint">{PROFILE_COPY.request.instantLine}</span>
          </>
        )}
        <DashButton variant="plainBlue" size="small" data-testid="profile-voting-info" onClick={() => setVotingInfo(true)}>
          {PROFILE_COPY.request.votingLink}
        </DashButton>
      </div>
      {votingInfo && <VotingInfoSheet onClose={() => setVotingInfo(false)} />}
    </section>
  );
}

function Outcomes({ walletId, identity, date }: { walletId: string; identity: string; date: (secs: number) => string }) {
  const events = useOutcomeEvents(walletId, identity);
  const rows = outcomeRows(events.data);
  if (rows.length === 0) return null;
  return (
    <section className="card-l1 profile-outcomes" data-testid="profile-outcomes" aria-label={PROFILE_COPY.outcomes.title}>
      <h2 className="card-title">{PROFILE_COPY.outcomes.title}</h2>
      <ul className="tool-list">
        {rows.map((r) => (
          <li key={r.id}>
            <span>{r.text}</span>
            <span className="field-hint">{date(r.at)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Ready({ walletId, network, main, contest, inputs }: { walletId: string; network: NetworkInfo; main: IdentitySummary; contest: ContestRef | null; inputs: StatusInputs }) {
  const b = useBackend();
  const flow = useModel(topUpFor(b, walletId));
  const costs = useCostTable(walletId);
  const pending = usePendingTopUps(walletId);
  const stuck = useStuckTopUps(walletId);
  const shown = useDisplay(network);
  useRefreshBalanceOnVisit(walletId, main.identity);
  const model = creditsModel(main, {
    costs: costs.data,
    pending: pending.data ?? [],
    stuck: stuck.data ?? [],
    toppingUp: flow.running || flow.rowRunning !== null,
    unseen: flow.unseenFor(main.identity),
    answeredUnknown: flow.answeredUnknown,
    amount: inputs.amount,
    masked: shown.masked(),
    discreet: inputs.discreet,
  });
  const rows = new Map((pending.data ?? []).map((p) => [p.txid, p]));
  return (
    <>
      <Header main={main} contest={contest?.label ?? null} walletId={walletId} />
      <Details main={main} contest={contest?.label ?? null} />
      {contest && <RequestCard walletId={walletId} main={main} contest={contest} inputs={inputs} />}
      <Credits model={model} flow={flow} rows={rows} inputs={inputs} onTopUp={() => flow.begin(main.identity)} />
      <Outcomes walletId={walletId} identity={main.identity} date={inputs.date} />
      <TopUpSheet flow={flow} walletId={walletId} network={network} costs={costs.data} />
    </>
  );
}

export function MyProfilePage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const { inputs, retry } = useStatusInputs(walletId, network);
  const page = pageModel(inputs);
  const onAction = (kind: CardActionKind) => {
    if (kind === "chooseName" && inputs.main) return chooseName(b, walletId, inputs.main.identity, false);
    if (kind === "buyCredits" && inputs.main) return buyCredits(b, walletId, inputs.main);
    return defaultCardAction(kind, retry);
  };
  return (
    <Page route="profile">
      <div className="profile-body" data-testid="profile-body" data-state={page.kind === "ready" ? (page.contest ? "contest" : "ready") : page.kind === "card" ? (page.noProfile ? "noProfile" : "error") : "loading"}>
        {page.kind === "loading" && (
          <p className="placeholder-note" role="status">
            {PROFILE_COPY.loading}
          </p>
        )}
        {page.kind === "card" && (
          <>
            {page.noProfile && <h2 className="profile-none">{PROFILE_COPY.noProfile}</h2>}
            <DashPayCard card={page.card} onAction={onAction} />
          </>
        )}
        {page.kind === "ready" && <Ready walletId={walletId} network={network} main={page.main} contest={page.contest} inputs={inputs} />}
      </div>
    </Page>
  );
}

/** Settings ▸ General's DashPay card (D20): My Profile with the identity's name, else Join. */
export function SettingsDashPayCard() {
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  const shell = useDashPayShell(wallet?.walletId);
  const joined = mainIdentityOf(shell.status) !== null;
  const name = shell.main?.mainName ?? null;
  return (
    <SettingsCard title="DashPay" testId="settings-dashpay">
      <div className="settings-row">
        <span className="settings-row-title" data-testid="settings-dashpay-who">
          {joined ? (name ? `@${name}` : PROFILE_COPY.settings.joined) : PROFILE_COPY.settings.notJoined}
        </span>
        <DashButton variant="tintedBlue" size="small" data-testid="settings-dashpay-open" disabled={!wallet} onClick={() => navigate(joined ? "profile" : "joinDashPay")}>
          {joined ? PROFILE_COPY.settings.profile : PROFILE_COPY.settings.join}
        </DashButton>
      </div>
    </SettingsCard>
  );
}
