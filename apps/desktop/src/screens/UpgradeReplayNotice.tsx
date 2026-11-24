import { useRescanProgress } from "../api/queries";
import { useT } from "../i18n";

/** Says why `walletId`'s sends wait while the first start after an update replays it from its birth. */
export function UpgradeReplayNotice({ walletId }: { walletId: string }) {
  const progress = useRescanProgress();
  const tr = useT();
  if (!progress.data?.upgradeWallets.includes(walletId)) return null;
  return (
    <section className="protocol-advisory-banner card-l1" data-testid="upgrade-replay-notice" aria-label={tr("app.upgradeReplay.label")}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <p className="protocol-advisory-text" data-testid="upgrade-replay-text">
          {tr("app.upgradeReplay.text")}
        </p>
      </div>
    </section>
  );
}
