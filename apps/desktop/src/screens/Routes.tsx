// The routes the shell opens (DASHPAY §4). Those later tasks build render their page with a
// placeholder, which U-02, U-04..U-06 and the DashPay tasks replace; Pay ▸ To an address is
// U-03's (screens/pay). What a link or a menu put on a
// route (Pay's prefill, Add contact's prefill, a Tools tab) is shown here already, so links
// route end to end.

import { useEffect, useId, useRef, type ReactNode } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { ui, useUi, type PayTab, type Route } from "../app/ui";
import { LoadingState } from "../components/history";
import { useT, type Translate } from "../i18n";
import { useDefaultPayTab } from "./pay/contactQueries";
import { PayToAddress } from "./pay/PayToAddress";
import { PayToContact } from "./pay/PayToContact";
import { ShieldedSendLink } from "./ShieldedSheet";
import { UpgradeReplayNotice } from "./UpgradeReplayNotice";

/** The placeholder text of each route that `Placeholder` renders. */
type NotYetRoute = Exclude<Route, "home" | "pay" | "receive" | "contacts" | "activity" | "coinJoin" | "tools" | "settings" | "notifications" | "joinDashPay" | "profile" | "masternodes">;

/** `wide`: the page's tables take the window's width, as Activity's table mode does. */
export function Page({ route, wide = false, badge, children }: { route: Route; wide?: boolean; badge?: ReactNode; children?: ReactNode }) {
  const t = useT();
  return (
    <main className={wide ? "page page-wide" : "page"} data-testid={`page-${route}`}>
      <div className="page-column">
        <header className="top-intro">
          <h1 className="top-intro-title">
            {t(`routes.${route}`)}
            {badge && " "}
            {badge}
          </h1>
        </header>
        {children}
      </div>
    </main>
  );
}

export function NotYet({ testId, children }: { testId: string; children: ReactNode }) {
  return (
    <p className="placeholder card-l1" data-testid={testId}>
      {children}
    </p>
  );
}

/** A page whose content comes with a later task. */
export function Placeholder({ route }: { route: NotYetRoute }) {
  const t = useT();
  return (
    <Page route={route}>
      <NotYet testId={`placeholder-${route}`}>{t(`app.notYet.${route}`)}</NotYet>
    </Page>
  );
}

export interface Tab<T extends string> {
  id: T;
  title: string;
}

/** The tab a key moves to from tab `i` of `n`, or `null`: ArrowRight goes to the tab drawn on the
 * right, which in a right-to-left tablist is the previous one. */
export function tabTarget(key: string, i: number, n: number, rtl: boolean): number | null {
  const step = rtl ? -1 : 1;
  const to: Record<string, number> = { ArrowRight: i + step, ArrowLeft: i - step, Home: 0, End: n - 1 };
  return key in to ? (to[key] + n) % n : null;
}

/** Tabs (WAI-ARIA tabs pattern): arrow keys move between them, in the tablist's direction. */
export function Tabs<T extends string>({
  label,
  tabs,
  current,
  onSelect,
  testId,
  children,
}: {
  label: string;
  tabs: Tab<T>[];
  current: T;
  onSelect: (t: T) => void;
  testId: string;
  children: ReactNode;
}) {
  const id = useId();
  const onKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    const i = tabs.findIndex((t) => t.id === current);
    const to = tabTarget(e.key, i, tabs.length, getComputedStyle(e.currentTarget).direction === "rtl");
    if (to === null) return;
    e.preventDefault();
    const next = tabs[to].id;
    onSelect(next);
    document.getElementById(`${id}-${next}`)?.focus();
  };
  return (
    <>
      <div className="tabs" role="tablist" aria-label={label} data-testid={testId} onKeyDown={onKeyDown}>
        {tabs.map((t) => (
          <button
            key={t.id}
            id={`${id}-${t.id}`}
            type="button"
            role="tab"
            className="tab"
            data-testid={`${testId}-${t.id}`}
            aria-selected={t.id === current}
            aria-controls={`${id}-panel`}
            tabIndex={t.id === current ? 0 : -1}
            onClick={() => onSelect(t.id)}
          >
            {t.title}
          </button>
        ))}
      </div>
      <section id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${current}`} className="tab-panel">
        {children}
      </section>
    </>
  );
}

const PAY_TABS = (tr: Translate): Tab<PayTab>[] => [
  { id: "contact", title: tr("app.pay.contact") },
  { id: "address", title: tr("app.pay.address") },
];

export function PayPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const tr = useT();
  const chosen = useUi((s) => s.payTab);
  const prefill = useUi((s) => s.payPrefill);
  const links = useUi((s) => s.payLinks);
  const coins = useUi((s) => s.payCoins);
  const auto = useDefaultPayTab(walletId);
  // Another wallet chooses its tab afresh, by its own contacts; a form a link or a tool filled
  // stays on its tab, so choosing the wallet that pays it keeps the payment in view.
  const shownWallet = useRef(walletId);
  const filled = prefill !== null || coins !== null;
  const rechoose = shownWallet.current !== walletId && !filled;
  useEffect(() => {
    shownWallet.current = walletId;
    if (rechoose) ui.set({ payTab: null });
  }, [rechoose, walletId]);
  const tab = rechoose ? null : (chosen ?? auto);
  // The tab picked for the user stays while Pay is open: contacts arriving later do not move it.
  useEffect(() => {
    if (chosen === null && auto !== null) ui.set({ payTab: auto });
  }, [chosen, auto]);
  return (
    <Page route="pay">
      <UpgradeReplayNotice walletId={walletId} />
      {tab === null ? (
        <LoadingState testId="pay-tab-loading" text={tr("app.pay.loading")} />
      ) : (
        <Tabs label={tr("app.pay.tabs")} tabs={PAY_TABS(tr)} current={tab} onSelect={(payTab) => ui.set({ payTab })} testId="pay-tabs">
          {tab === "address" ? (
            // Keyed by the links that opened Pay and the wallet, so each one starts a fresh form.
            <PayToAddress key={`${links}:${walletId}`} prefill={prefill} coins={coins} walletId={walletId} network={network} />
          ) : (
            <PayToContact walletId={walletId} network={network} />
          )}
        </Tabs>
      )}
      <ShieldedSendLink walletId={walletId} network={network} />
    </Page>
  );
}

// Contacts is DP2-07's (screens/contacts); the link card moved into its detail slot.
export { ContactsPage } from "./contacts/ContactsPage";
