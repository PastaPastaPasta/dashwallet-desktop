import { DashButton } from "../components/basics";
import { useDismissUpgradeNotice, useUpgradeNotice } from "../api/queries";
import { useT } from "../i18n";
import { textFor } from "./StoreRefusal";

/** The store upgrade's notice (Q107 107-1): the wallet opened after an upgrade without a before-upgrade
 * copy it can restore. It blocks nothing; Dismiss hides it for this page and changes nothing on disk. */
export function UpgradeNoticeBanner() {
  const notice = useUpgradeNotice().data;
  const dismiss = useDismissUpgradeNotice();
  const tr = useT();
  if (!notice) return null;
  return (
    <section className="protocol-advisory-banner card-l1" data-testid="upgrade-notice" data-key={notice.key}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <p className="protocol-advisory-text" data-testid="upgrade-notice-text">
          {textFor(tr, notice.key, notice.text, "")}
        </p>
      </div>
      <div className="protocol-advisory-actions">
        <DashButton variant="tintedGray" size="small" data-testid="upgrade-notice-dismiss" onClick={dismiss}>
          {tr("storeUpgrade.notice.dismiss")}
        </DashButton>
      </div>
    </section>
  );
}
