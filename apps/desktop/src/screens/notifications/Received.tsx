// Incoming payments in the shell (DEC-1058 #6): the app-wide "Received X DASH" toast, and the
// bell's Payments section (popover and page). The rows are payments/received.ts's.

import { useEffect, useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useLockStatus } from "../../api/queries";
import { useUi } from "../../app/ui";
import { dismissPrivateToast, showToast } from "../../components/toast";
import { useDisplay, type Display } from "../../format/display";
import { detailDate } from "../../format/tx";
import { exposedNow, markReceivedRead, onReceived, useReceived, type ReceivedPayment } from "../../payments/received";

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

/** Whether `el` is on screen for someone: connected, and with no `inert` ancestor (the shell is inert
 * behind a sheet, a dialog, the lock screen, onboarding). It follows the attribute as it changes. */
function useExposed(el: HTMLElement | null): boolean {
  const [exposed, setExposed] = useState(false);
  useEffect(() => {
    if (!el) {
      setExposed(false);
      return;
    }
    const check = () => setExposed(exposedNow(el));
    check();
    const watch = new MutationObserver(check);
    watch.observe(document.documentElement, { attributes: true, attributeFilter: ["inert"], subtree: true });
    return () => watch.disconnect();
  }, [el]);
  return exposed;
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
  // Seen means shown, to someone: the rows on screen, with the wallet unlocked and nothing over the
  // view (a page can stay mounted behind the lock screen or a transaction's sheet).
  const locked = useLockStatus().data?.state === "locked";
  const [section, setSection] = useState<HTMLElement | null>(null);
  const exposed = useExposed(section);
  const shownIds = rows.map((p) => p.id).join(",");
  useEffect(() => {
    // `exposed` re-runs this when a cover closes; what authorizes the marking is the DOM now, since a
    // row and a covering sheet can commit together, before the observer has told `exposed`.
    if (locked || !exposed || shownIds === "" || !exposedNow(section)) return;
    // A sheet that opens into the layer covers the shell one render after it mounts: ask again once
    // that has happened, so a row that committed with it is not marked read behind it.
    const ids = shownIds.split(",");
    const later = setTimeout(() => {
      if (exposedNow(section)) markReceivedRead(walletId, ids);
    }, 0);
    return () => clearTimeout(later);
  }, [walletId, locked, exposed, section, shownIds]);
  if (payments.length === 0) return null;
  return (
    <section ref={setSection} className="bell-section" aria-labelledby={titleId} data-testid="bell-payments">
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
