import { useId, useState } from "react";
import { useBackend, useShieldedStatus } from "../api/queries";
import { DashButton } from "../components/basics";
import { useT } from "../i18n";
import { useModel } from "../onboarding/model";
import { SHIELDED_CLOSED_TEXT, shieldedClosedReason } from "../shielded/flow";
import { FailedLink } from "./settings/About";
import { OpenFailureActions } from "./StoreRefusal";

/** Home's notice while the upgrade's move of the old Shielded records stopped: Shielded stays closed
 * this session and says why; the rest of the wallet is open. Restart only where a start may open it
 * (another program held the data, or the move failed); Report a problem, with its warning, where the
 * user needs support. Gone once Shielded opens. */
export function ShieldedClosedNotice({ walletId }: { walletId: string }) {
  const tr = useT();
  const backend = useBackend();
  const [actions] = useState(() => new OpenFailureActions(backend));
  useModel(actions);
  const titleId = useId();
  const reason = shieldedClosedReason(useShieldedStatus(walletId).error);
  if (reason === null) return null;
  const restart = reason === "in_use" || reason === "failed";
  const report = reason !== "in_use";
  return (
    <section className="protocol-advisory-banner history-gap-notice card-l1" data-testid="shielded-closed-notice" data-reason={reason} aria-labelledby={titleId}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <h2 className="history-gap-title" id={titleId}>
          {tr("transfer.shielded.closed.title")}
        </h2>
        <p className="protocol-advisory-text" data-testid="shielded-closed-text">
          {tr(SHIELDED_CLOSED_TEXT[reason])}
        </p>
        {actions.problem && (
          <p className="field-error" role="alert" data-testid="shielded-closed-error">
            {actions.problem}
          </p>
        )}
        {actions.failedLink && (
          <p className="protocol-advisory-text" data-testid="shielded-closed-link">
            <FailedLink url={actions.failedLink} />
          </p>
        )}
        <div className="history-gap-actions">
          {restart && (
            <DashButton variant="tintedBlue" size="small" data-testid="shielded-closed-restart" disabled={actions.running} onClick={() => void actions.restart()}>
              {tr("transfer.shielded.closed.restart")}
            </DashButton>
          )}
          {report && (
            <DashButton variant={restart ? "plainBlue" : "tintedBlue"} size="small" data-testid="shielded-closed-report" disabled={actions.running} onClick={() => void actions.report()}>
              {tr("settings.about.links.issues")}
            </DashButton>
          )}
        </div>
        {report && (
          <p className="field-hint" data-testid="shielded-closed-issues-hint">
            {tr("settings.about.links.issuesHint")}
          </p>
        )}
      </div>
    </section>
  );
}
