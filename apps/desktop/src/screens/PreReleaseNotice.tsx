// The first-run early-release notice (0.1 release plan): one screen over the app, dismissed once
// and then kept dismissed in the settings file (`preReleaseNoticeDismissed`). It is the same
// window onboarding uses, so it reads as the first step.

import { DashButton, DashLogo } from "../components/basics";

export const PreReleaseText = {
  title: "Dash Wallet 0.1 is an early release",
  body: "It has had no security review; use small amounts.",
  dismiss: "I understand",
};

export function PreReleaseNotice({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div className="onboarding pre-release" role="alertdialog" aria-modal="true" aria-labelledby="pre-release-title" aria-describedby="pre-release-body" data-testid="pre-release-notice">
      <div className="onboarding-panel">
        <header className="onboarding-header">
          <span className="onboarding-header-side" />
          <span className="onboarding-brand">
            <DashLogo className="onboarding-logo" />
          </span>
          <span className="onboarding-header-side" />
        </header>
        <main className="onboarding-body">
          <h1 className="onboarding-title" id="pre-release-title">
            {PreReleaseText.title}
          </h1>
          <p className="onboarding-text" id="pre-release-body">
            {PreReleaseText.body}
          </p>
          <DashButton variant="filledBlue" size="large" fill autoFocus data-testid="pre-release-dismiss" onClick={onDismiss}>
            {PreReleaseText.dismiss}
          </DashButton>
        </main>
      </div>
    </div>
  );
}
