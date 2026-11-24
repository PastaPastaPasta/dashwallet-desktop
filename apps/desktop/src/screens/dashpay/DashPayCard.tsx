// The DashPay status card (DASHPAY F1, DP1-08): one card per banner state, on Home under the
// shortcut card and on the Join page. The model is dashpay/status.ts `homeCard`.

import { useId, useState, type ReactNode } from "react";
import type { Backend } from "../../api/backend";
import { navigate, ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import type { CardActionKind, CardModel } from "../../dashpay/status";
import { useT } from "../../i18n";
import { transferFor } from "../../transfer/model";
import { VotingInfoSheet } from "./AboutDashPay";

const ICONS: Record<CardModel["tone"], ReactNode> = {
  problem: (
    <>
      <circle cx="10" cy="10" r="7.5" />
      <path d="M10 6v5M10 13.5v.5" />
    </>
  ),
  pending: (
    <>
      <circle cx="10" cy="10" r="7.5" />
      <path d="M10 5.5V10l3 2" />
    </>
  ),
  quiet: (
    <>
      <circle cx="10" cy="7" r="3" />
      <path d="M4 17c.6-3 3-4.5 6-4.5s5.4 1.5 6 4.5" />
    </>
  ),
};

function CardIcon({ tone }: { tone: CardModel["tone"] }) {
  return (
    <svg className="dashpay-card-icon" viewBox="0 0 20 20" aria-hidden="true">
      {ICONS[tone]}
    </svg>
  );
}

/** "Move to Dash wallet" from a held Platform payment (#685): Home's transfer, which plans beside
 * the held payment from its inputs only, so it is that payment's way out. */
export function moveToDashWallet(b: Backend, walletId: string) {
  navigate("home");
  transferFor(b, walletId).start("platformToCore");
}

/** The card's buttons outside the Join page: into the Join page, the unlock sheet, Tools ▸ Repair,
 * Home's move to the Dash wallet. */
export function defaultCardAction(kind: CardActionKind, retry: () => void, moveToCore: () => void) {
  switch (kind) {
    case "join":
    case "continue":
    case "chooseName":
      return navigate("joinDashPay");
    case "retry":
      return retry();
    case "unlock":
      return ui.set({ dialog: "unlock" });
    case "repair":
      return navigate("tools", { toolsTab: "repair" });
    case "moveToCore":
      return moveToCore();
    // Home opens the top-up sheet before it navigates (Overview.tsx); elsewhere, My Profile.
    case "buyCredits":
      return navigate("profile");
    // The card opens its own sheet.
    case "votingInfo":
      return;
  }
}

export function DashPayCard({ card, onAction }: { card: CardModel; onAction: (kind: CardActionKind) => void }) {
  const tr = useT();
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  const { action, secondary, step } = card;
  return (
    <section
      className={`dashpay-card card-l1 is-${card.tone}`}
      data-testid="dashpay-card"
      data-state={card.state}
      aria-labelledby={`${id}-title`}
    >
      <CardIcon tone={card.tone} />
      <div className="dashpay-card-body">
        {/* Changes as the registration or the vote moves on: the title (its username), the step and
            the lines, announced politely (D25). */}
        <div role="status" className="dashpay-card-live">
          <h2 className="dashpay-card-title" id={`${id}-title`}>
            {card.title}
            {card.busy && <span className="spinner dashpay-card-spinner" aria-hidden="true" />}
          </h2>
          {step && (
            <div className="dashpay-card-step" data-testid="dashpay-card-step">
              <span>{tr("dashpay.step.shown", { step: `${step.n}/3`, text: step.text })}</span>
              <progress className="dashpay-card-progress" max={3} value={step.n} aria-label={tr("dashpay.step.progress")} />
            </div>
          )}
          <div data-testid="dashpay-card-lines">
            {card.lines.map((line) => (
              <p key={line} className="dashpay-card-line">
                {line}
              </p>
            ))}
          </div>
        </div>
        {card.note && (
          <p className="dashpay-card-note" data-testid="dashpay-card-note">
            {card.note}
          </p>
        )}
      </div>
      {action && (
        <div className="dashpay-card-actions">
          <DashButton
            variant={action.kind === "votingInfo" ? "plainBlue" : "filledBlue"}
            size="small"
            className="dashpay-card-action"
            data-testid="dashpay-card-action"
            disabled={action.disabled}
            onClick={() => (action.kind === "votingInfo" ? setVotingInfo(true) : onAction(action.kind))}
          >
            {action.label}
          </DashButton>
          {secondary && (
            <DashButton
              variant="tintedBlue"
              size="small"
              className="dashpay-card-action"
              data-testid="dashpay-card-secondary"
              disabled={secondary.disabled}
              onClick={() => onAction(secondary.kind)}
            >
              {secondary.label}
            </DashButton>
          )}
        </div>
      )}
      {votingInfo && <VotingInfoSheet onClose={() => setVotingInfo(false)} />}
    </section>
  );
}
