import { useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, useProtocolCompatibility } from "../api/queries";
import { DashButton } from "../components/basics";
import {
  dismissProtocolAdvisory,
  PROTOCOL_ADVISORY_COPY as C,
  RELEASES_URL,
  shouldShowProtocolAdvisory,
  useProtocolAdvisoryDismissed,
} from "../protocol/advisory";

export function ProtocolAdvisoryBanner({ network }: { network: NetworkInfo }) {
  const backend = useBackend();
  const compat = useProtocolCompatibility();
  const dismissed = useProtocolAdvisoryDismissed(network.dirName);
  const [problem, setProblem] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);

  if (compat.isError || !shouldShowProtocolAdvisory(compat.data, dismissed)) return null;

  return (
    <section
      className="protocol-advisory-banner card-l1"
      data-testid="protocol-advisory-banner"
      aria-label={C.label}
    >
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body">
        <p className="protocol-advisory-text" data-testid="protocol-advisory-text">
          {C.text}
        </p>
        {problem && (
          <p className="field-error" role="alert" data-testid="protocol-advisory-error">
            {problem}
          </p>
        )}
      </div>
      <div className="protocol-advisory-actions">
        <a
          href={RELEASES_URL}
          className="protocol-advisory-link"
          data-testid="protocol-advisory-link"
          aria-disabled={opening ? "true" : undefined}
          onClick={(e) => {
            // Remote navigation is blocked in the webview (U6); dw-app opens the OS browser.
            e.preventDefault();
            if (opening) return;
            setProblem(null);
            setOpening(true);
            void backend
              .openReleasesPage()
              .catch(() => setProblem(C.openFailed))
              .finally(() => setOpening(false));
          }}
        >
          {C.link}
        </a>
        <DashButton
          variant="tintedGray"
          size="small"
          data-testid="protocol-advisory-dismiss"
          onClick={() => {
            dismissProtocolAdvisory(network.dirName);
            requestAnimationFrame(() => {
              if (document.activeElement === document.body) {
                document.querySelector<HTMLElement>('[data-testid="hero-amount"]')?.focus();
              }
            });
          }}
        >
          {C.dismiss}
        </DashButton>
      </div>
    </section>
  );
}
