import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { errorMessage } from "../api/backend";
import { keys, useBackend, useDroppedNotices } from "../api/queries";
import { navigate, ui, useUi } from "../app/ui";
import { AmountText, DashButton } from "../components/basics";
import { useDisplay } from "../format/display";
import { recipientOf } from "../history/dropped";
import { useT } from "../i18n";

/** Home's notice of payments Dash Wallet dropped because the Dash they used came from an incoming
 * transaction that was never valid. The engine lists only those whose coins are back; Dismiss hides
 * them for good. Details opens the payment, or Activity when there are several. */
export function DroppedNoticeBanner({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const tr = useT();
  const b = useBackend();
  const client = useQueryClient();
  const notices = useDroppedNotices(walletId);
  const display = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const rows = (notices.data ?? []).filter((n) => recipientOf(n) !== null);
  if (rows.length === 0) return null;
  const txids = [...new Set(rows.map((n) => n.txid))];
  // A payment to several payees has a row for each: one notice names them all, with its total.
  const one = txids.length === 1 ? { txid: txids[0], amount: rows.reduce((sum, n) => sum + n.amount, 0), recipient: rows.map(recipientOf).join(", ") } : null;
  const amount = (duffs: number) => <AmountText text={discreet ? display.masked() : display.full(duffs, false)} unitName={display.name} glyph={false} />;
  const dismiss = async () => {
    setBusy(true);
    setProblem(null);
    try {
      await b.dismissDroppedNotices(walletId, txids);
      await client.invalidateQueries({ queryKey: keys.droppedNotices(walletId) });
    } catch (e) {
      setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="protocol-advisory-banner card-l1" data-testid="dropped-notice" aria-label={tr("activity.status.droppedLabel")}>
      <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 6v5M10 13.5v.5" />
      </svg>
      <div className="protocol-advisory-body" role="status">
        <p className="protocol-advisory-text" data-testid="dropped-notice-text">
          {one
            ? tr.rich("activity.invalidSource.notice", { amount: amount(one.amount), recipient: one.recipient })
            : tr("activity.invalidSource.noticeSeveral", { count: txids.length })}
        </p>
        {problem && (
          <p className="field-error" role="alert" data-testid="dropped-notice-error">
            {problem}
          </p>
        )}
      </div>
      <div className="protocol-advisory-actions">
        <button
          type="button"
          className="protocol-advisory-link"
          data-testid="dropped-notice-details"
          onClick={() => (one ? ui.set({ detail: one.txid }) : navigate("activity"))}
        >
          {tr("activity.invalidSource.details")}
        </button>
        <DashButton variant="tintedGray" size="small" data-testid="dropped-notice-dismiss" disabled={busy} onClick={() => void dismiss()}>
          {tr("activity.invalidSource.dismiss")}
        </DashButton>
      </div>
    </section>
  );
}
