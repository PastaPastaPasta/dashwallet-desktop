// My Profile (DASHPAY §4 `MyProfileViewModel`, F5, F7, F8; DP1-10 P1–P6): the header, usernames,
// identity id, the credits block with its warnings, its pending or stuck top-ups and its recent
// withdrawals (DP6-02), the own contest's card while one runs, and the contest outcome rows.
// Without a main identity it is DP1-08's card for the state (D2). The view models are
// dashpay/profile.ts, dashpay/topup.ts and dashpay/withdraw.ts;
// the Settings card (D20) is here too. Edit Profile (DP4-03) is a sheet over the page.

import { Fragment, useEffect, useId, useRef, useState, type ReactNode } from "react";
import type { Backend } from "../../api/backend";
import type { IdentitySummary } from "../../api/generated/IdentitySummary";
import type { PendingTopUp } from "../../api/generated/PendingTopUp";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { mainIdentityOf, useBackend, useDashPayShell, useWallets } from "../../api/queries";
import { navigate, setLeaveGuard, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { creditsModel, headerModel, holdsTopUp, outcomeRows, pageModel, PROFILE_COPY, requestModel, type ContestRef, type CreditsModel } from "../../dashpay/profile";
import { editProfileFor, type EditProfileModel } from "../../dashpay/editProfile";
import { useAvatarUploadAvailable, useContactPictures, useContest, useCostTable, useOutcomeEvents, usePendingTopUps, useProfileLimits, useRefreshBalanceOnVisit, useStuckTopUps } from "../../dashpay/profileQueries";
import { useStatusInputs } from "../../dashpay/queries";
import { myQr } from "../../contacts/add";
import { useMyUserLink } from "../../contacts/queries";
import type { CardActionKind, StatusInputs } from "../../dashpay/status";
import { outcomeText, topUpErrorText, topUpFor, type TopUpFlow } from "../../dashpay/topup";
import { withdrawFor } from "../../dashpay/withdraw";
import { wizardFor } from "../../dashpay/wizard";
import { useDisplay } from "../../format/display";
import { t, useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { Page } from "../Routes";
import { SettingsCard } from "../settings/parts";
import { showToast } from "../../components/toast";
import { EditProfileSheet } from "./EditProfileSheet";
import { VotingInfoSheet } from "./AboutDashPay";
import { DashPayCard, defaultCardAction, moveToDashWallet } from "./DashPayCard";
import { TopUpSheet } from "./TopUpSheet";
import { FROM_PLATFORM_COPY } from "../../dashpay/topupSource";
import { INSTANT_COPY, VOTE_COPY } from "../../dashpay/instantOffer";
import { ShareSheet } from "./ShareSheet";
import { RecentWithdrawals, WithdrawButton, WithdrawSheet } from "./WithdrawSheet";

/** Opens DP1-09's credits path for `identity` on the Join page; `instant` refuses a contested
 * label (D13). `label` prefills the field (Try again with a temporary name that failed). */
export function chooseName(b: Backend, walletId: string, identity: string, instant: boolean, label = "") {
  const wiz = wizardFor(b, walletId);
  wiz.begin({ kind: "credits", identity, instant });
  if (label !== "") wiz.setLabel(label);
  navigate("joinDashPay");
}

/** The voting card's instant actions (D4 reversed): D13's instant username, or Try again with the
 * temporary name the registration could not register. `false` for another action. */
export function instantCardAction(b: Backend, walletId: string, main: IdentitySummary | undefined, kind: CardActionKind, card: { retryLabel?: string } | null): boolean {
  if (!main || (kind !== "instantName" && kind !== "instantRetry")) return false;
  chooseName(b, walletId, main.identity, true, kind === "instantRetry" ? (card?.retryLabel ?? "") : "");
  return true;
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

export function ProfileHeader({ main, contest, walletId, onEdit }: { main: IdentitySummary; contest: string | null; walletId: string; onEdit: () => void }) {
  useT();
  const b = useBackend();
  const h = headerModel(main, contest);
  const [sharing, setSharing] = useState(false);
  const share = useRef<HTMLButtonElement>(null);
  const edit = useRef<HTMLButtonElement>(null);
  const link = useMyUserLink(walletId, main.identity, h.shareable);
  const canShare = h.shareable && myQr(link, main.mainName).kind === "link";
  useEffect(() => {
    if (!h.shareable) setSharing(false);
  }, [h.shareable]);
  return (
    <section className="profile-header card-l1" data-testid="profile-header">
      <Avatar letters={h.initials} size="pane" identity={main.identity} own />
      <div className="profile-header-text">
        <h2 className="profile-name" data-testid="profile-name">
          {h.title}
        </h2>
        <p className="profile-username" data-testid="profile-username" data-names={h.names}>
          {h.names === "named" ? <bdi dir="ltr">{h.username}</bdi> : h.username}
        </p>
        {h.message && <p className="profile-message">{h.message}</p>}
        {h.names === "none" && contest === null && (
          <DashButton variant="tintedBlue" size="small" data-testid="profile-choose-name" onClick={() => chooseName(b, walletId, main.identity, false)}>
            {PROFILE_COPY.chooseName}
          </DashButton>
        )}
      </div>
      <div className="profile-edit">
        <DashButton variant="tintedBlue" size="small" data-testid="profile-edit" ref={edit} onClick={onEdit}>
          {PROFILE_COPY.editProfile}
        </DashButton>
        {canShare && (
          <DashButton variant="tintedBlue" size="small" data-testid="profile-share" ref={share} onClick={() => setSharing(true)}>
            {PROFILE_COPY.share}
          </DashButton>
        )}
      </div>
      {sharing && h.shareable && (
        <ShareSheet
          walletId={walletId}
          identity={main.identity}
          who={{ name: main.profile?.displayName?.trim() || null, username: main.mainName! }}
          testId="profile-share-sheet"
          returnFocus={() => share.current ?? edit.current}
          onClose={() => setSharing(false)}
        />
      )}
    </section>
  );
}

function Details({ main, contest }: { main: IdentitySummary; contest: string | null }) {
  useT();
  return (
    <section className="card-l1 profile-details" data-testid="profile-details" aria-label={PROFILE_COPY.identity}>
      <dl className="detail-list">
        <DetailRow label={PROFILE_COPY.usernames} testId="profile-usernames">
          {main.names.length > 0
            ? main.names.map((n, i) => (
                <Fragment key={n}>
                  {i > 0 && t("common.listSeparator")}
                  <bdi dir="ltr">{`@${n}`}</bdi>
                </Fragment>
              ))
            : headerModel(main, contest).username}
        </DetailRow>
        <DetailRow label={PROFILE_COPY.identity} testId="profile-identity">
          <CopyValue value={main.identity} action={PROFILE_COPY.copyIdentityId} mono />
        </DetailRow>
      </dl>
    </section>
  );
}

/** `withdraw`: DP6-02's button beside Top up and its rows under the top-ups (D9, D11). */
function Credits({
  model,
  flow,
  rows,
  inputs,
  onTopUp,
  withdraw,
}: {
  model: CreditsModel;
  flow: TopUpFlow;
  rows: Map<string, PendingTopUp>;
  inputs: StatusInputs;
  onTopUp: () => void;
  withdraw: { button: ReactNode; rows: ReactNode };
}) {
  useT();
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
            <bdi>{model.figure}</bdi>
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
      {withdraw.rows}
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
        {withdraw.button}
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
  useT();
  const b = useBackend();
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  const read = useContest(walletId, main.identity, contest.label);
  const m = requestModel(contest.label, contest.endsAt, read.data, read.isError, main, inputs.date, inputs.instant);
  const topUp = useModel(topUpFor(b, walletId));
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
        {m.short && (
          <DashButton variant="tintedBlue" size="small" data-testid="profile-instant-top-up" onClick={() => topUp.begin(main.identity)}>
            {VOTE_COPY.topUp}
          </DashButton>
        )}
        {m.failed !== null && (
          <DashButton variant="tintedBlue" size="small" data-testid="profile-instant-retry" onClick={() => chooseName(b, walletId, main.identity, true, m.failed ?? "")}>
            {INSTANT_COPY.retry}
          </DashButton>
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
  useT();
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
  useT();
  const b = useBackend();
  const flow = useModel(topUpFor(b, walletId));
  const withdraw = useModel(withdrawFor(b, walletId));
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
  const editor = useEditor(walletId);
  return (
    <>
      <ProfileHeader main={main} contest={contest?.label ?? null} walletId={walletId} onEdit={() => editor.begin(main.identity, main.profile)} />
      <Details main={main} contest={contest?.label ?? null} />
      {contest && <RequestCard walletId={walletId} main={main} contest={contest} inputs={inputs} />}
      <Credits
        model={model}
        flow={flow}
        rows={rows}
        inputs={inputs}
        onTopUp={() => flow.begin(main.identity)}
        withdraw={{
          button: <WithdrawButton flow={withdraw} identity={main.identity} balance={main.balance} />,
          rows: <RecentWithdrawals flow={withdraw} walletId={walletId} identity={main.identity} costs={costs.data} amount={inputs.amount} date={inputs.date} />,
        }}
      />
      <Outcomes walletId={walletId} identity={main.identity} date={inputs.date} />
      <TopUpSheet flow={flow} walletId={walletId} network={network} costs={costs.data} name={main.mainName ? `@${main.mainName}` : FROM_PLATFORM_COPY.you} />
      <WithdrawSheet flow={withdraw} walletId={walletId} network={network} costs={costs.data} />
      <EditProfileSheet model={editor} letters={headerModel(main, contest?.label ?? null).initials} username={main.mainName} />
    </>
  );
}

/** The wallet's Edit Profile (DP4-03 D1), fed the engine's limits and picture sources (D5, D7,
 * D13), and the leave guard while the page shows (D11). */
function useEditor(walletId: string): EditProfileModel {
  const b = useBackend();
  const editor = editProfileFor(b, walletId, showToast);
  const limits = useProfileLimits(walletId);
  const upload = useAvatarUploadAvailable(walletId);
  const pictures = useContactPictures();
  useEffect(() => editor.setLimits(limits.data), [editor, limits.data]);
  useEffect(() => editor.setAvailability(upload.data === true, pictures.data === true), [editor, upload.data, pictures.data]);
  useEffect(() => setLeaveGuard((route, go) => route !== "profile" && editor.requestLeave(go)), [editor]);
  return editor;
}

export function MyProfilePage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  useT();
  const b = useBackend();
  const { inputs, retry } = useStatusInputs(walletId, network);
  const page = pageModel(inputs);
  const onAction = (kind: CardActionKind) => {
    if (kind === "chooseName" && inputs.main) return chooseName(b, walletId, inputs.main.identity, false);
    if (kind === "buyCredits" && inputs.main) return buyCredits(b, walletId, inputs.main);
    if (page.kind === "card" && instantCardAction(b, walletId, inputs.main, kind, page.card)) return;
    return defaultCardAction(kind, retry, () => moveToDashWallet(b, walletId));
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
  useT();
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  const shell = useDashPayShell(wallet?.walletId);
  const joined = mainIdentityOf(shell.status) !== null;
  const name = shell.main?.mainName ?? null;
  return (
    <SettingsCard title={PROFILE_COPY.settings.title} testId="settings-dashpay">
      <div className="settings-row">
        <span className="settings-row-title" data-testid="settings-dashpay-who">
          {joined ? (name ? <bdi dir="ltr">{`@${name}`}</bdi> : PROFILE_COPY.settings.joined) : PROFILE_COPY.settings.notJoined}
        </span>
        <DashButton variant="tintedBlue" size="small" data-testid="settings-dashpay-open" disabled={!wallet} onClick={() => navigate(joined ? "profile" : "joinDashPay")}>
          {joined ? PROFILE_COPY.settings.profile : PROFILE_COPY.settings.join}
        </DashButton>
      </div>
    </SettingsCard>
  );
}
