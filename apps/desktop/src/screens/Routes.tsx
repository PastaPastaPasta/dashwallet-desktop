// The routes the shell opens (DASHPAY §4). Those later tasks build render their page with a
// placeholder, which U-02, U-04..U-06 and the DashPay tasks replace; Pay ▸ To an address is
// U-03's (screens/pay). What a link or a menu put on a
// route (Pay's prefill, Add contact's prefill, a Tools tab) is shown here already, so links
// route end to end.

import { useId, type ReactNode } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { ui, useUi, type PayTab, type Route } from "../app/ui";
import { PayToAddress } from "./pay/PayToAddress";

const TITLES: Record<Route, string> = {
  home: "Home",
  pay: "Pay",
  receive: "Receive",
  contacts: "Contacts",
  activity: "Activity",
  coinJoin: "CoinJoin",
  explore: "Explore",
  notifications: "Notifications",
  profile: "My Profile",
  settings: "Settings",
  tools: "Tools",
  joinDashPay: "Join DashPay",
};

/** The placeholder text of each route that `Placeholder` renders. */
const NOT_YET: Record<Exclude<Route, "home" | "pay" | "receive" | "contacts" | "activity" | "tools" | "settings" | "notifications" | "joinDashPay">, string> = {
  coinJoin: "CoinJoin mixing is not available yet.",
  explore: "Explore is not available yet.",
  profile: "Your DashPay profile is not available yet.",
};

/** `wide`: the page's tables take the window's width, as Activity's table mode does. */
export function Page({ route, wide = false, children }: { route: Route; wide?: boolean; children?: ReactNode }) {
  return (
    <main className={wide ? "page page-wide" : "page"} data-testid={`page-${route}`}>
      <div className="page-column">
        <header className="top-intro">
          <h1 className="top-intro-title">{TITLES[route]}</h1>
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
export function Placeholder({ route }: { route: keyof typeof NOT_YET }) {
  return (
    <Page route={route}>
      <NotYet testId={`placeholder-${route}`}>{NOT_YET[route]}</NotYet>
    </Page>
  );
}

export interface Tab<T extends string> {
  id: T;
  title: string;
}

/** Tabs (WAI-ARIA tabs pattern): arrow keys move between them. */
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
  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = tabs.findIndex((t) => t.id === current);
    const to: Record<string, number> = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: tabs.length - 1 };
    if (!(e.key in to)) return;
    e.preventDefault();
    const next = tabs[(to[e.key] + tabs.length) % tabs.length].id;
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

const PAY_TABS: Tab<PayTab>[] = [
  { id: "contact", title: "To a contact" },
  { id: "address", title: "To an address" },
];

export function PayPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const tab = useUi((s) => s.payTab);
  const prefill = useUi((s) => s.payPrefill);
  const links = useUi((s) => s.payLinks);
  const coins = useUi((s) => s.payCoins);
  return (
    <Page route="pay">
      <Tabs label="Pay" tabs={PAY_TABS} current={tab} onSelect={(payTab) => ui.set({ payTab })} testId="pay-tabs">
        {tab === "address" ? (
          // Keyed by the links that opened Pay and the wallet, so each one starts a fresh form.
          <PayToAddress key={`${links}:${walletId}`} prefill={prefill} coins={coins} walletId={walletId} network={network} />
        ) : (
          <NotYet testId="pay-contact">Paying a DashPay contact is not available yet.</NotYet>
        )}
      </Tabs>
    </Page>
  );
}

// Contacts is DP2-07's (screens/contacts); the link card moved into its detail slot.
export { ContactsPage } from "./contacts/ContactsPage";
