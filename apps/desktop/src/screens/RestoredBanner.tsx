import { useRestored } from "../api/queries";
import { useT } from "../i18n";

/** While the wallet data restored from before the upgrade is in use and RESTORED is set (until the
 * engine's first full sync): what is missing, and that anything that moves funds is paused. */
export function RestoredBanner() {
  const restored = useRestored();
  const tr = useT();
  if (restored.data !== true) return null;
  return (
    <section className="protocol-advisory-banner card-l1" data-testid="restored-banner" aria-label={tr("storeUpgrade.restore.banner")}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <p className="protocol-advisory-text" data-testid="restored-banner-paused">
          {tr("storeUpgrade.restore.banner")}
        </p>
        <p className="protocol-advisory-text" data-testid="restored-banner-text">
          {tr("storeUpgrade.restored.text")}
        </p>
      </div>
    </section>
  );
}
