// Incoming payments in the shell (DEC-1058 #6): the app-wide "Received X DASH" toast, and the
// bell's Payments section (popover and page). The rows are payments/received.ts's.

import { useEffect, useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useLockStatus } from "../../api/queries";
import { useUi } from "../../app/ui";
import { dismissPrivateToast, showToast } from "../../components/toast";
import { useDisplay, type Display } from "../../format/display";
import { detailDate } from "../../format/tx";
import { markReceivedRead, onReceived, useReceived, type ReceivedPayment } from "../../payments/received";

/** "Received 0.01 DASH"; discreet mode does not say how much. */
export function receivedText(duffs: number, shown: Display, discreet: boolean): string {
  return discreet ? "Received a payment" : `Received ${shown.compact(duffs)} ${shown.name}`;
}

/** The toast for every payment that arrives, wherever the window is (one per batch, its total). It holds
 * both texts, so Discreet mode masks it while it is up; locking takes it down, and none comes while locked. */
export function ReceivedToaster({ network }: { network: NetworkInfo }) {
  const shown = useDisplay(network);
  const locked = useLockStatus().data?.state === "locked";
  useEffect(() => {
    if (locked) {
      dismissPrivateToast();
      return;
    }
    return onReceived((batch) => {
      const total = batch.reduce((n, p) => n + p.duffs, 0);
      showToast(receivedText(total, shown, false), undefined, receivedText(total, shown, true));
    });
  }, [shown, locked]);
  return null;
}

/** The bell's "Payments": what arrived since the app opened, the ones not seen before marked new.
 * Opening it marks them seen. Nothing shows while none came. */
export function ReceivedSection({ walletId, network, limit, onOpen }: { walletId: string; network: NetworkInfo; limit?: number; onOpen: (txid: string) => void }) {
  const payments = useReceived(walletId);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const titleId = useId();
  // The payments seen before the view opened: any other, including one that arrives while it is
  // open, is marked new until the view closes.
  const [seen] = useState<ReadonlySet<string>>(() => new Set(payments.filter((p) => p.read).map((p) => p.id)));
  const rows: readonly ReceivedPayment[] = limit ? payments.slice(0, limit) : payments;
  // Seen means shown, to someone: the rows on screen, with the wallet unlocked (a page can stay mounted
  // behind the lock screen).
  const locked = useLockStatus().data?.state === "locked";
  const shownIds = rows.map((p) => p.id).join(",");
  useEffect(() => {
    if (!locked && shownIds !== "") markReceivedRead(walletId, shownIds.split(","));
  }, [walletId, locked, shownIds]);
  if (payments.length === 0) return null;
  return (
    <section className="bell-section" aria-labelledby={titleId} data-testid="bell-payments">
      <h3 className="bell-section-title" id={titleId}>
        Payments
      </h3>
      <ul className="bell-rows">
        {rows.map((p) => {
          const fresh = !seen.has(p.id);
          return (
            <li key={p.id} className={fresh ? "bell-row is-new" : "bell-row"} data-testid="bell-payment" data-txid={p.txid}>
              <button type="button" className="bell-row-open" onClick={() => onOpen(p.txid)}>
                {fresh && <span className="bell-dot" aria-hidden="true" />}
                <span className="bell-row-text">
                  <span className="bell-row-title">{receivedText(p.duffs, shown, discreet)}</span>
                  <span className="bell-row-sub">{detailDate(new Date(p.at))}</span>
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
