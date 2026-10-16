// Join DashPay (DASHPAY §4 `JoinDashPayViewModel`, F1; DP1-08): the intro (iOS
// `JoinDashPayScreen`), the readiness under it by status (J1–J7), the voting info and the FAQ.
// Create username opens the registration wizard (DP1-09, Registration.tsx); while a registration
// goes on, one stopped with its funds committed, or this session's has a result to show, the page
// is its screen instead (D1, D12, D22).
// Routes.tsx re-exports the other pages; this one is imported directly, as it uses Routes' `Page`.

import { useEffect, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { useRegistrations, useStatusInputs, useUsernameQuote } from "../../dashpay/queries";
import { namedModel, pendingModel, screenModel, type ScreenActionKind } from "../../dashpay/registration";
import { JOIN_COPY, joinModel, type CardActionKind } from "../../dashpay/status";
import { wizardFor } from "../../dashpay/wizard";
import { useModel } from "../../onboarding/model";
import { Page } from "../Routes";
import { DashPayFaqSheet, VotingInfoSheet } from "./AboutDashPay";
import { DashPayCard, defaultCardAction } from "./DashPayCard";
import { RegistrationScreen, WizardSteps } from "./Registration";

function Intro() {
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
  const invitation = useUi((s) => s.invitation);
  const b = useBackend();
  const wiz = useModel(wizardFor(b, walletId));
  const { inputs, retry } = useStatusInputs(walletId, network);
  const { status } = inputs;
  const joining = status?.kind === "noIdentity" && status.reason !== "watchOnly";
  const quote = useUsernameQuote(walletId, joining);
  const model = joinModel(inputs, quote?.totalDuffs ?? null, quote?.availableDuffs ?? null);
  const [sheet, setSheet] = useState<"voting" | "faq" | null>(null);
  // The row shown: this session's (until Done), else the one the status says goes on (D12), else
  // one that stopped for good with its funds committed, which the status no longer names but
  // which still blocks a start (D22).
  const rows = useRegistrations(walletId);
  const stopped = rows.data?.findLast((r) => r.phase === "failed" && r.fundsCommitted && r.failure?.retryable === false);
  const draft = wiz.draft ?? (status?.kind === "registering" ? status.draft : status?.kind === "noIdentity" ? (stopped?.draft ?? null) : null);
  const row = draft === null ? undefined : rows.data?.find((r) => r.draft === draft);
  const shown = row?.draft;
  useEffect(() => {
    if (shown !== undefined) wiz.adopt(shown);
  }, [wiz, shown]);

  const onAction = (kind: CardActionKind) => {
    if (kind === "chooseName" && inputs.main) return wiz.begin({ kind: "credits", identity: inputs.main.identity });
    return defaultCardAction(kind, retry);
  };
  const onScreenAction = (kind: ScreenActionKind) => {
    switch (kind) {
      case "home":
        return navigate("home");
      case "unlock":
      case "repair":
        return defaultCardAction(kind, retry);
      case "continue":
        return void (row && wiz.resume(row));
      case "finish":
        return void wiz.finish();
      case "another":
        return void wiz.another(row ?? null);
      case "done":
        return wiz.close();
    }
  };

  if (wiz.step !== null)
    return (
      <Page route="joinDashPay">
        <WizardSteps wiz={wiz} walletId={walletId} inputs={inputs} />
      </Page>
    );
  if (wiz.named)
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={namedModel(wiz.named.label, wiz.named.result, inputs.date)} onAction={onScreenAction} inputs={inputs} />
      </Page>
    );
  // Started this session, its row not read yet (D23): never the Join page, which offers Create.
  if (wiz.draft !== null && !row)
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={pendingModel()} onAction={onScreenAction} inputs={inputs} />
      </Page>
    );
  if (row) {
    const contest = status?.kind === "contestPending" && status.label === row.label ? status : null;
    const screen = screenModel(row, { contestPending: contest !== null, contestEndsAt: contest?.endsAt ?? null, date: inputs.date, amount: inputs.amount, discreet: inputs.discreet });
    return (
      <Page route="joinDashPay">
        <RegistrationScreen wiz={wiz} model={screen} phase={row.phase} waiting={row.waiting} onAction={onScreenAction} inputs={inputs} />
      </Page>
    );
  }

  return (
    <Page route="joinDashPay">
      {invitation && (
        <p className="placeholder-note" role="status" data-testid="invitation-received">
          You opened a DashPay invitation. Joining with an invitation is not available yet: keep the link and open it again once
          it is.
        </p>
      )}
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
              <DashButton variant="filledBlue" data-testid="join-create" disabled={model.waiting} onClick={() => wiz.begin({ kind: "core" })}>
                {JOIN_COPY.create}
              </DashButton>
              {model.waiting && inputs.status && (
                <span className="dashpay-card-note" data-testid="join-waiting">
                  {JOIN_COPY.waiting}
                </span>
              )}
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
    </Page>
  );
}
