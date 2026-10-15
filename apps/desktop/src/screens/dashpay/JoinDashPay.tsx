// Join DashPay (DASHPAY §4 `JoinDashPayViewModel`, F1; DP1-08): the intro (iOS
// `JoinDashPayScreen`), the readiness under it by status (J1–J7), the voting info and the FAQ.
// Create username leads to the registration wizard, DP1-09's: a placeholder until then (D7).
// Routes.tsx re-exports the other pages; this one is imported directly, as it uses Routes' `Page`.

import { useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBalance } from "../../api/queries";
import { useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { useStatusInputs, useUsernameCost } from "../../dashpay/queries";
import { JOIN_COPY, joinModel, type CardActionKind } from "../../dashpay/status";
import { NotYet, Page } from "../Routes";
import { DashPayFaqSheet, VotingInfoSheet } from "./AboutDashPay";
import { DashPayCard, defaultCardAction } from "./DashPayCard";

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
  const { inputs, retry } = useStatusInputs(walletId, network);
  const joining = inputs.status?.kind === "noIdentity" && inputs.status.reason !== "watchOnly";
  const cost = useUsernameCost(walletId, joining);
  const balance = useBalance(walletId);
  const model = joinModel(inputs, cost, balance.data?.total ?? null);
  const [wizard, setWizard] = useState(false);
  const [sheet, setSheet] = useState<"voting" | "faq" | null>(null);

  const onAction = (kind: CardActionKind) => (kind === "continue" || kind === "chooseName" ? setWizard(true) : defaultCardAction(kind, retry));

  if (wizard)
    return (
      <Page route="joinDashPay">
        <NotYet testId="placeholder-registration">{JOIN_COPY.wizard}</NotYet>
        <DashButton variant="plainBlue" data-testid="join-wizard-back" onClick={() => setWizard(false)}>
          {JOIN_COPY.back}
        </DashButton>
      </Page>
    );

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
              <DashButton variant="filledBlue" data-testid="join-create" disabled={model.waiting} onClick={() => setWizard(true)}>
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
