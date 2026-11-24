// Release collateral (MG-10 §1.7, MG-00 `upd-release`): a checkbox, then `release_collateral`.
// Sends nothing, so no grant. Lock it again is the pane's own action (EP 1's `collateralReleased`).

import { useState } from "react";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { isAppError } from "../../api/backend";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useT, type Translate } from "../../i18n";
import { mnErrorText, rowName } from "../../masternodes/model";
import { collateralDash, registrationCalls, registrationSends } from "../../masternodes/registration";
import { useMasternodesRefresh } from "../../masternodes/queries";

const L = "masternodes.release";

/** `collateral_unavailable` has its own sentence here (§1.7). */
export const collateralErrorText = (e: unknown, t: Translate) =>
  isAppError(e) && e.code === "masternodes.collateral_unavailable" ? t(`${L}.unavailable`) : mnErrorText(e);

export function ReleaseSheet({ row, wallet, network, onClose }: { row: MasternodeRow; wallet: WalletSummary; network: NetworkInfo; onClose: () => void }) {
  const t = useT();
  const b = registrationCalls(useBackend(), registrationSends(network));
  const refresh = useMasternodesRefresh();
  const [understood, setUnderstood] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = rowName(row);
  const release = async () => {
    setBusy(true);
    setError(null);
    try {
      await b.mnReleaseCollateral(wallet.walletId, row.proTxHash);
      await refresh();
      onClose();
    } catch (e) {
      setError(collateralErrorText(e, t));
      setBusy(false);
    }
  };
  return (
    <Sheet title={t(`${L}.title`)} width="medium" testId="mn-release-sheet" closable={!busy} onClose={onClose}>
      <form
        className="mg-sheet-body"
        onSubmit={(e) => {
          e.preventDefault();
          void release();
        }}
      >
        <p>{t(`${L}.text`, { name, amount: collateralDash(row.kind ?? "regular") })}</p>
        <p className="mg-notice mg-notice-danger" data-testid="mn-release-warning">
          {t(`${L}.warning`)}
        </p>
        <label className="mg-check">
          <input type="checkbox" data-testid="mn-release-understood" checked={understood} disabled={busy} onChange={(e) => setUnderstood(e.target.checked)} />
          <span>{t(`${L}.confirm`, { name })}</span>
        </label>
        <p className="mg-small mg-muted">{t(`${L}.note`)}</p>
        <p className="form-problem" role="alert" data-testid="mn-release-error">
          {error}
        </p>
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" onClick={onClose} disabled={busy}>
              {t(`${L}.cancel`)}
            </DashButton>
            <DashButton type="submit" variant="plainRed" size="medium" data-testid="mn-release-confirm" disabled={busy || !understood}>
              {t(`${L}.release`)}
            </DashButton>
          </span>
        </footer>
      </form>
    </Sheet>
  );
}
