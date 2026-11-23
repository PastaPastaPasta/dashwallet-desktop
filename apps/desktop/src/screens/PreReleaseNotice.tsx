// The first-run early-release notice (0.1 release plan): one screen over the app, dismissed once
// and then kept dismissed in the settings file (`preReleaseNoticeDismissed`). It is the same
// window onboarding uses, so it reads as the first step.

import { DashButton, DashLogo } from "../components/basics";
import { useT } from "../i18n";

export function PreReleaseNotice({ onDismiss }: { onDismiss: () => void }) {
  const t = useT();
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
            {t("onboarding.preRelease.title")}
          </h1>
          <p className="onboarding-text" id="pre-release-body">
            {t("onboarding.preRelease.body")}
          </p>
          <DashButton variant="filledBlue" size="large" fill autoFocus data-testid="pre-release-dismiss" onClick={onDismiss}>
            {t("onboarding.preRelease.dismiss")}
          </DashButton>
        </main>
      </div>
    </div>
  );
}
