// R9, the welcome moment (PRODUCT #4): a registration's success, with its three next steps under
// the greeting. Share my username opens ShareSheet.

import { REG_COPY, WELCOME_STEPS, type WelcomeStepKind } from "../../dashpay/registration";

/** `canEdit`: the main identity and its profile are read. `canShare`: the engine has the link for the username shown. */
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
            <button type="button" className="welcome-step" id={`welcome-${s.kind}`} data-testid={`welcome-${s.kind}`} disabled={disabled || waiting(s.kind)} aria-labelledby={`welcome-${s.kind}-title`} aria-describedby={`welcome-${s.kind}-detail`} onClick={() => onStep(s.kind)}>
              <span className="welcome-step-number" aria-hidden="true">
                {i + 1}
              </span>
              <span className="welcome-step-text">
                <span className="welcome-step-title" id={`welcome-${s.kind}-title`}>
                  {s.title}
                </span>
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
