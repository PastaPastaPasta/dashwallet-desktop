// Join DashPay (DASHPAY §4 `JoinDashPayViewModel`, F1; DP1-08): the intro (iOS
// `JoinDashPayScreen`), the readiness under it by status (J1–J7), the voting info and the FAQ.
// Create username opens the registration wizard (DP1-09, Registration.tsx) at its "Make your
// username private" step (C-funding F16, F19); while a registration
// goes on, one stopped with its funds committed, or this session's has a result to show, the page
// is its screen instead (D1, D12, D22). An invitation opened (`ui.invitation`, DP5-03) shows the
// claim screen (Claim.tsx) in place of the intro; its claim is the wizard's invitation mode.
// Routes.tsx re-exports the other pages; this one is imported directly, as it uses Routes' `Page`.

import { useEffect, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, usePoolBalances } from "../../api/queries";
import { navigate, ui, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CLAIM_COPY, claimHasIdentity, claimRow } from "../../dashpay/claim";
import { useNamesRead, useRegistrations, useShieldedReadiness, useStatusInputs, useUsernameQuote } from "../../dashpay/queries";
import { temporaryOutcome } from "../../dashpay/instantOffer";
import { namedModel, pendingModel, screenModel, type ScreenActionKind } from "../../dashpay/registration";
import { JOIN_COPY, joinModel, shieldedFunds, type CardActionKind } from "../../dashpay/status";
import { findFriends, openEditProfile } from "../../dashpay/welcome";
import { wizardFor } from "../../dashpay/wizard";
import { known } from "../../format/homeBalance";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { Page } from "../Routes";
import { DashPayFaqSheet, VotingInfoSheet } from "./AboutDashPay";
import { ClaimConfirmStep, ClaimScreen, InvitationPasteSheet } from "./Claim";
import { DashPayCard, defaultCardAction, moveToDashWallet } from "./DashPayCard";
import { chooseName, identityCardAction } from "./MyProfile";
import { RegistrationScreen, WizardSteps } from "./Registration";

function Intro() {
  useT();
  return (
    <div className="join-intro card-l1" data-testid="join-intro">
      <h2 className="join-intro-headline">{JOIN_COPY.headline}</h2>
      <p className="join-intro-pitch">{JOIN_COPY.pitch}</p>
      <ul className="join-features" role="list">
        {JOIN_COPY.features.map((f) => (
          <li key={f.title} className="join-feature">
            <span className="join-feature-title">{f.title}</span>
            <span className="join-feature-text">{f.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function JoinDashPayPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  useT();
  const invitation = useUi((s) => s.invitation);
  const b = useBackend();
  const wiz = useModel(wizardFor(b, walletId));
  const { inputs, retry } = useStatusInputs(walletId, network);
  const { status } = inputs;
  const joining = status?.kind === "noIdentity" && status.reason !== "watchOnly";
  const quote = useUsernameQuote(walletId, joining);
  const readiness = useShieldedReadiness(walletId, joining);
  const pools = usePoolBalances(walletId);
  const model = joinModel(inputs, quote?.totalDuffs ?? null, quote?.availableDuffs ?? null, shieldedFunds(readiness, known(pools.shielded) ? pools.shielded : null));
  const [sheet, setSheet] = useState<"voting" | "faq" | "invitation" | null>(null);
  // The row shown: this session's (until Done), else the one the status says goes on (D12), else
  // one that stopped for good with its funds committed, which the status no longer names but
  // which still blocks a start (D22).
  const rows = useRegistrations(walletId);
  const stopped = rows.data?.findLast((r) => r.phase === "failed" && r.fundsCommitted && r.failure?.retryable === false);
  // DP5-03 D5: the invitation's own claim first, whatever its status says.
  const claim = invitation ? claimRow(rows.data, invitation.linkId) : undefined;
  const draft =
    wiz.draft ?? claim?.draft ?? (status?.kind === "registering" ? status.draft : status?.kind === "noIdentity" ? (stopped?.draft ?? null) : null);
  const row = draft === null ? undefined : rows.data?.find((r) => r.draft === draft);
  // What became of the row's temporary name, from a names read since it reached its contest.
  const namesRead = useNamesRead(walletId, row);
  // An instant username registered this session from credits (D13) while the own vote runs.
  const instantFor =
    wiz.named?.result.kind === "registered" && wiz.mode.kind === "credits" && wiz.mode.instant === true && status?.kind === "contestPending" ? status.label : null;
  const shown = row?.draft;
  useEffect(() => {
    if (shown !== undefined) wiz.adopt(shown);
  }, [wiz, shown]);
  // A draft of this session's missing from a read (D23) is read once more; still missing (it was
  // discarded elsewhere), the page is the Join page again (review r2 L2).
  const missing = wiz.draft !== null && !wiz.named && rows.isSuccess && !row ? wiz.draft : null;
  const { refetch } = rows;
  useEffect(() => {
    if (missing === null) return;
    let live = true;
    void refetch().then((r) => {
      if (live && r.isSuccess && wiz.draft === missing && !r.data.some((x) => x.draft === missing)) wiz.close();
    });
    return () => {
      live = false;
    };
  }, [wiz, missing, refetch]);

  const moveToCore = () => moveToDashWallet(b, walletId);
  const onAction = (kind: CardActionKind) => {
    if (kind === "chooseName" && inputs.main) return wiz.begin({ kind: "credits", identity: inputs.main.identity });
    if (identityCardAction(b, walletId, inputs.main, kind, model.kind === "card" ? model.card : null)) return;
    return defaultCardAction(kind, retry, moveToCore);
  };
  // A claim seen through leaves the claim screen too.
  const leave = () => {
    if (row?.invitation) ui.set({ invitation: null });
    wiz.close();
  };
  const onScreenAction = (kind: ScreenActionKind) => {
    switch (kind) {
      case "home":
        return navigate("home");
      case "unlock":
      case "repair":
      case "moveToCore":
        return defaultCardAction(kind, retry, moveToCore);
      case "continue":
        return void (row && wiz.resume(row));
      case "finish":
        return void wiz.finish();
      case "another":
        return void wiz.another(row ?? null);
      case "instantRetry":
        if (!row?.identity || !row.temporaryLabel) return;
        wiz.close();
        return chooseName(b, walletId, row.identity, true, row.temporaryLabel);
      case "reconfirm":
        return void (row && wiz.reconfirm(row));
      case "done":
        return leave();
      case "wallet":
        leave();
        return navigate("home");
      case "editProfile":
        leave();
        return openEditProfile(b, walletId, inputs.main);
      case "findFriends":
        leave();
        return findFriends(walletId);
    }
  };
  const shareIdentity = row?.identity ?? (wiz.mode.kind === "credits" ? wiz.mode.identity : null) ?? inputs.main?.identity ?? null;
  // The link names the identity's main name, which a second name registered does not replace: an
  // existing identity's new name is never shown as the one shared.
  const shareMain = inputs.main?.identity === shareIdentity ? inputs.main.mainName : null;
  const shareName = shareMain ?? (wiz.mode.kind === "credits" ? null : (wiz.named?.label ?? row?.label ?? null));
  const share = shareIdentity === null || shareName === null ? null : { walletId, identity: shareIdentity, username: shareName };

  if (wiz.step !== null)
    return (
      <Page route="joinDashPay">
        {wiz.step === "confirm" && wiz.mode.kind === "invitation" ? (
          <ClaimConfirmStep wiz={wiz} walletId={walletId} inputs={inputs} />
        ) : (
          <WizardSteps wiz={wiz} walletId={walletId} inputs={inputs} network={network.kind} />
        )}
      </Page>
    );
  if (wiz.named)
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={namedModel(wiz.named.label, wiz.named.result, inputs.date, instantFor)} onAction={onScreenAction} inputs={inputs} share={share} />
      </Page>
    );
  // Started this session, its row not read yet (D23): never the Join page, which offers Create.
  if (wiz.draft !== null && !row)
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={pendingModel(rows.isError)} onAction={onScreenAction} inputs={inputs} />
      </Page>
    );
  if (row) {
    const contest = status?.kind === "contestPending" && status.label === row.label ? status : null;
    const screen = screenModel(row, {
      contestPending: contest !== null,
      contestEndsAt: contest?.endsAt ?? null,
      date: inputs.date,
      amount: inputs.amount,
      discreet: inputs.discreet,
      temporary: temporaryOutcome(row, namesRead),
    });
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={screen} phase={row.phase} waiting={row.waiting} claim={row.invitation !== null} fromPlatform={row.fromPlatform} onAction={onScreenAction} inputs={inputs} share={share} />
      </Page>
    );
  }

  if (invitation) {
    // C3: an identity of the wallet (a registration under way shows its row above), or one a claim
    // was refused for, not found yet (D24).
    const hasIdentity = claimHasIdentity(status?.kind === "ready" || status?.kind === "contestPending", rows.data);
    return (
      <Page route="joinDashPay">
        <ClaimScreen linkId={invitation.linkId} entry={invitation.entry} wiz={wiz} hasIdentity={hasIdentity} network={network.dirName} rows={rows} />
      </Page>
    );
  }

  return (
    <Page route="joinDashPay">
      <Intro />
      <div className="join-readiness" data-testid="join-readiness" data-kind={model.kind}>
        {model.kind === "join" && (
          <>
            <p className="join-line" data-testid="join-cost">
              {model.cost}
            </p>
            {model.shortfall && (
              <p className="join-line join-shortfall" data-testid="join-shortfall">
                {model.shortfall}
              </p>
            )}
            <div className="join-actions">
              <DashButton variant="filledBlue" data-testid="join-create" disabled={model.waiting} onClick={() => wiz.begin({ kind: "core" }, "source")}>
                {JOIN_COPY.create}
              </DashButton>
              {model.waiting && inputs.status && (
                <span className="dashpay-card-note" data-testid="join-waiting">
                  {JOIN_COPY.waiting}
                </span>
              )}
              <DashButton variant="plainBlue" data-testid="join-invitation" onClick={() => setSheet("invitation")}>
                {CLAIM_COPY.paste.open}
              </DashButton>
            </div>
          </>
        )}
        {model.kind === "watchOnly" && (
          <p className="join-line" data-testid="join-watch-only">
            {JOIN_COPY.watchOnly}
          </p>
        )}
        {model.kind === "card" && <DashPayCard card={model.card} onAction={onAction} />}
        {model.kind === "joined" && (
          <p className="join-line" data-testid="join-joined">
            {JOIN_COPY.joined(model.username)}
          </p>
        )}
      </div>
      <div className="join-links">
        <DashButton variant="plainBlue" size="small" data-testid="join-voting-info" onClick={() => setSheet("voting")}>
          {JOIN_COPY.votingLink}
        </DashButton>
        <DashButton variant="plainBlue" size="small" data-testid="join-faq" onClick={() => setSheet("faq")}>
          {JOIN_COPY.faqLink}
        </DashButton>
      </div>
      {sheet === "voting" && <VotingInfoSheet onClose={() => setSheet(null)} />}
      {sheet === "faq" && <DashPayFaqSheet onClose={() => setSheet(null)} />}
      {sheet === "invitation" && <InvitationPasteSheet onClose={() => setSheet(null)} />}
    </Page>
  );
}
