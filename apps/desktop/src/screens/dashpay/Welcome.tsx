// R9, the welcome moment (PRODUCT #4): a registration's success, with its three next steps under
// the greeting. Share my username opens Add contact's My QR in a sheet.

import { Sheet } from "../../components/sheet";
import { REG_COPY, SHARE_TITLE, WELCOME_STEPS, type WelcomeStepKind } from "../../dashpay/registration";
import { MyQrTab } from "../contacts/AddContact";

/** `canEdit`: the main identity and its profile are read. `canShare`: whose username to share is known. */
export function WelcomeSteps({
  onStep,
  disabled,
  canEdit,
  canShare,
}: {
  onStep: (kind: WelcomeStepKind) => void;
  disabled: boolean;
  canEdit: boolean;
  canShare: boolean;
}) {
  const waiting = (kind: WelcomeStepKind) => (kind === "editProfile" && !canEdit) || (kind === "share" && !canShare);
  return (
    <div className="welcome-next" data-testid="welcome-steps">
      <h3 className="welcome-next-title">{REG_COPY.done.next}</h3>
      <ol className="welcome-steps" role="list">
        {WELCOME_STEPS.map((s, i) => (
          <li key={s.kind}>
            <button type="button" className="welcome-step" data-testid={`welcome-${s.kind}`} disabled={disabled || waiting(s.kind)} aria-describedby={`welcome-${s.kind}-detail`} onClick={() => onStep(s.kind)}>
              <span className="welcome-step-number" aria-hidden="true">
                {i + 1}
              </span>
              <span className="welcome-step-text">
                <span className="welcome-step-title">{s.title}</span>
                <span className="welcome-step-detail" id={`welcome-${s.kind}-detail`}>{s.text}</span>
              </span>
              <span className="welcome-step-chevron" aria-hidden="true">
                ›
              </span>
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}

export function ShareSheet({ walletId, identity, onClose }: { walletId: string; identity: string; onClose: () => void }) {
  return (
    <Sheet title={SHARE_TITLE} width="small" testId="welcome-share-sheet" onClose={onClose}>
      <MyQrTab walletId={walletId} identity={identity} />
    </Sheet>
  );
}
