import { useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, useProtocolCompatibility } from "../api/queries";
import { DashButton } from "../components/basics";
import { useT } from "../i18n";
import {
  dismissProtocolAdvisory,
  shouldShowProtocolAdvisory,
  useProtocolAdvisoryDismissed,
} from "../protocol/advisory";

function focusHomeAfterDismiss() {
  if (document.activeElement !== document.body) return;
  const hero = document.querySelector<HTMLElement>('[data-testid="hero-amount"]');
  if (hero && hero.tabIndex >= 0) {
    hero.focus();
    return;
  }
  const shortcut = document.querySelector<HTMLElement>('[data-testid="shortcut-receive"]');
  if (shortcut && shortcut.tabIndex >= 0) {
    shortcut.focus();
    return;
  }
  const page = document.querySelector<HTMLElement>('[data-testid="page-home"]');
  if (!page) return;
  page.tabIndex = -1;
  page.focus();
}

export function ProtocolAdvisoryBanner({ network }: { network: NetworkInfo }) {
  const backend = useBackend();
  const compat = useProtocolCompatibility();
  const dismissed = useProtocolAdvisoryDismissed(network.dirName);
  const tr = useT();
  const [problem, setProblem] = useState(false);
  const [opening, setOpening] = useState(false);

  if (compat.isError || !shouldShowProtocolAdvisory(compat.data, dismissed)) return null;

  return (
    <section
      className="protocol-advisory-banner card-l1"
      data-testid="protocol-advisory-banner"
      aria-label={tr("home.advisory.label")}
    >
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <p className="protocol-advisory-text" data-testid="protocol-advisory-text">
          {tr("home.advisory.text")}
        </p>
        {problem && (
          <p className="field-error" role="alert" data-testid="protocol-advisory-error">
            {tr("home.advisory.openFailed")}
          </p>
        )}
      </div>
      <div className="protocol-advisory-actions">
        <button
          type="button"
          className="protocol-advisory-link"
          data-testid="protocol-advisory-link"
          disabled={opening}
          onClick={() => {
            setProblem(false);
            setOpening(true);
            void backend
              .openReleasesPage()
              .catch(() => setProblem(true))
              .finally(() => setOpening(false));
          }}
        >
          {tr("home.advisory.link")}
        </button>
        <DashButton
          variant="tintedGray"
          size="small"
          data-testid="protocol-advisory-dismiss"
          onClick={() => {
            dismissProtocolAdvisory(network.dirName);
            requestAnimationFrame(focusHomeAfterDismiss);
          }}
        >
          {tr("home.advisory.dismiss")}
        </DashButton>
      </div>
    </section>
  );
}
