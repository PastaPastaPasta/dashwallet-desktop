// Sync progress (IOS-023, QT-025/QT-027): the status bar's sync item opens this sheet, which lists
// each SPV phase with its own progress (the status bar shows only their mean), the latest block and
// its ChainLock, the connected peers, and how long the sync has gone without progress.

import type { SyncPhaseStatus } from "../api/generated/SyncPhaseStatus";
import type { SyncStatus } from "../api/generated/SyncStatus";
import { useBackend, useSyncStatus } from "../api/queries";
import { DetailRow } from "../components/detail";
import { Sheet } from "../components/sheet";
import { detailDate } from "../format/tx";
import { syncState } from "../format/sync";
import { useT } from "../i18n";

/** A stall the sheet calls out: no progress for this long while not caught up. */
export const STALL_SECS = 60;

type Translate = ReturnType<typeof useT>;

/** One phase's line: done, waiting for a target, or "current of target (percent)". */
export function phaseText(t: Translate, p: SyncPhaseStatus, locale: string | undefined): string {
  if (p.done) return t("shell.sync.done");
  if (p.targetHeight === null || p.targetHeight <= 0) return t("shell.sync.waiting");
  const current = Math.min(p.currentHeight ?? 0, p.targetHeight);
  const n = new Intl.NumberFormat(locale);
  const percent = new Intl.NumberFormat(locale, { style: "percent", minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(Math.min(current / p.targetHeight, 0.999));
  return t("shell.sync.progress", { current: n.format(current), target: n.format(p.targetHeight), percent });
}

/** Minutes without progress worth saying, or `null`. */
export function stalledMinutes(s: SyncStatus): number | null {
  if (!s.running || s.caughtUp || s.secondsSinceProgress === null || s.secondsSinceProgress < STALL_SECS) return null;
  return Math.floor(s.secondsSinceProgress / 60);
}

export function SyncSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const { locale } = useBackend();
  const sync = useSyncStatus().data;
  const n = new Intl.NumberFormat(locale);
  const stalled = sync ? stalledMinutes(sync) : null;
  return (
    <Sheet title={t("shell.status.syncProgress")} width="small" testId="sync-sheet" onClose={onClose}>
      <p className="sheet-text" data-testid="sync-sheet-state">
        {syncState(sync)}
      </p>
      {stalled !== null && (
        <p className="sheet-warning" role="note" data-testid="sync-sheet-stalled">
          {t("shell.sync.stalled", { count: stalled })}
        </p>
      )}
      {sync && (
        <dl className="detail-list card-l1" data-testid="sync-sheet-phases">
          {sync.phases.map((p) => (
            <DetailRow key={p.phase} label={t(`shell.sync.phase.${p.phase}`)} testId={`sync-phase-${p.phase}`}>
              <span className={sync.activePhase === p.phase && !p.done ? "sync-phase-active" : undefined}>{phaseText(t, p, locale)}</span>
            </DetailRow>
          ))}
          {sync.tipHeight !== null && (
            <DetailRow label={t("shell.sync.latestBlock")} testId="sync-tip">
              {sync.tipTime !== null
                ? t("shell.sync.latestBlockValue", { height: n.format(sync.tipHeight), time: detailDate(new Date(sync.tipTime * 1000), locale) })
                : n.format(sync.tipHeight)}
            </DetailRow>
          )}
          {sync.chainlockHeight !== null && (
            <DetailRow label={t("shell.sync.chainlock")} testId="sync-chainlock">
              {n.format(sync.chainlockHeight)}
            </DetailRow>
          )}
          <DetailRow label={t("shell.sync.peers")} testId="sync-peers">
            {n.format(sync.connectedPeers)}
          </DetailRow>
        </dl>
      )}
    </Sheet>
  );
}
