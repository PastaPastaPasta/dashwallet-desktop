// The routes the shell opens that later tasks build (DASHPAY §4): each renders its page with a
// placeholder, which U-02..U-06 and the DashPay tasks replace. What a link or a menu put on a
// route (Pay's prefill, Add contact's prefill, a Tools tab) is shown here already, so links
// route end to end.

import { useId, useState, type ReactNode } from "react";
import { DashButton } from "../components/basics";
import { full } from "../format/amount";
import { navigate, ui, useUi, type PayPrefill, type PayTab, type Route, type ToolsTab } from "../app/ui";

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
const NOT_YET: Record<Exclude<Route, "home" | "pay" | "contacts" | "activity" | "tools">, string> = {
  receive: "Receiving addresses and payment requests are not available yet.",
  coinJoin: "CoinJoin mixing is not available yet.",
  explore: "Explore is not available yet.",
  notifications: "DashPay notifications are not available yet.",
  profile: "Your DashPay profile is not available yet.",
  settings: "Settings are not available yet.",
  joinDashPay: "Creating a DashPay username is not available yet.",
};

function Page({ route, children }: { route: Route; children?: ReactNode }) {
  return (
    <main className="page" data-testid={`page-${route}`}>
      <div className="page-column">
        <header className="top-intro">
          <h1 className="top-intro-title">{TITLES[route]}</h1>
        </header>
        {children}
      </div>
    </main>
  );
}

function NotYet({ testId, children }: { testId: string; children: ReactNode }) {
  return (
    <p className="placeholder card-l1" data-testid={testId}>
      {children}
    </p>
  );
}

/** A page whose content comes with a later task. */
export function Placeholder({ route }: { route: keyof typeof NOT_YET }) {
  const invitation = useUi((s) => s.invitation);
  return (
    <Page route={route}>
      {route === "joinDashPay" && invitation && (
        <p className="placeholder-note" role="status" data-testid="invitation-received">
          You opened a DashPay invitation. Joining with an invitation is not available yet: keep the link and open it
          again once it is.
        </p>
      )}
      <NotYet testId={`placeholder-${route}`}>{NOT_YET[route]}</NotYet>
    </Page>
  );
}

interface Tab<T extends string> {
  id: T;
  title: string;
}

/** Tabs (WAI-ARIA tabs pattern): arrow keys move between them. */
function Tabs<T extends string>({
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

function Field({ label, value, testId, mono }: { label: string; value: string; testId: string; mono?: boolean }) {
  const id = useId();
  const [text, setText] = useState(value);
  return (
    <div className="form-field">
      <label htmlFor={id}>{label}</label>
      <input id={id} className={mono ? "mono" : undefined} data-testid={testId} value={text} onChange={(e) => setText(e.target.value)} />
    </div>
  );
}

function PayToAddress({ prefill }: { prefill: PayPrefill | null }) {
  return (
    <form className="pay-form" data-testid="pay-address-form" onSubmit={(e) => e.preventDefault()}>
      {prefill?.username && (
        <p className="placeholder-note" data-testid="pay-username">
          The link names the payee @{prefill.username}. The name is not verified.
        </p>
      )}
      <Field label="Address" value={prefill?.address ?? ""} testId="pay-address" mono />
      <Field label="Amount (DASH)" value={prefill?.amount ? full(prefill.amount, false) : ""} testId="pay-amount" />
      <Field label="Label" value={prefill?.label ?? ""} testId="pay-label" />
      <Field label="Message" value={prefill?.message ?? ""} testId="pay-message" />
      <NotYet testId="placeholder-pay">Sending is not available yet.</NotYet>
      <DashButton variant="filledWhiteBlue" disabled data-testid="pay-continue">
        Continue
      </DashButton>
    </form>
  );
}

const PAY_TABS: Tab<PayTab>[] = [
  { id: "contact", title: "To a contact" },
  { id: "address", title: "To an address" },
];

export function PayPage() {
  const tab = useUi((s) => s.payTab);
  const prefill = useUi((s) => s.payPrefill);
  const links = useUi((s) => s.payLinks);
  return (
    <Page route="pay">
      <Tabs label="Pay" tabs={PAY_TABS} current={tab} onSelect={(payTab) => ui.set({ payTab })} testId="pay-tabs">
        {tab === "address" ? (
          // Keyed by the links that opened Pay, so each one resets what was typed.
          <PayToAddress key={links} prefill={prefill} />
        ) : (
          <NotYet testId="pay-contact">Paying a DashPay contact is not available yet.</NotYet>
        )}
      </Tabs>
    </Page>
  );
}

export function ContactsPage() {
  const prefill = useUi((s) => s.contactPrefill);
  return (
    <Page route="contacts">
      {prefill && (
        <section className="add-contact card-l1" aria-labelledby="add-contact-title" data-testid="add-contact">
          <h2 id="add-contact-title" className="add-contact-title">
            Add contact
          </h2>
          <p data-testid="add-contact-username">@{prefill.username}</p>
          <p className="mono" data-testid="add-contact-identity">
            {prefill.identity}
          </p>
          <p className="placeholder-note">The username comes from the link and is not verified yet.</p>
          <span className="add-contact-actions">
            <DashButton variant="filledWhiteBlue" disabled>
              Send contact request
            </DashButton>
            <DashButton variant="plainBlue" onClick={() => ui.set({ contactPrefill: null })}>
              Dismiss
            </DashButton>
          </span>
        </section>
      )}
      <NotYet testId="placeholder-contacts">Your DashPay contacts are not available yet.</NotYet>
    </Page>
  );
}

const TOOLS_TABS: Tab<ToolsTab>[] = [
  { id: "information", title: "Information" },
  { id: "console", title: "Console" },
  { id: "peers", title: "Peers" },
  { id: "repair", title: "Repair" },
  { id: "addressBook", title: "Address Book" },
  { id: "signVerify", title: "Sign/Verify" },
  { id: "psbt", title: "PSBT" },
  { id: "coinSelection", title: "Coin Selection" },
];

export function ToolsPage() {
  const tab = useUi((s) => s.toolsTab);
  const title = TOOLS_TABS.find((t) => t.id === tab)!.title;
  return (
    <Page route="tools">
      <Tabs label="Tools" tabs={TOOLS_TABS} current={tab} onSelect={(toolsTab) => navigate("tools", { toolsTab })} testId="tools-tabs">
        <NotYet testId={`placeholder-tools-${tab}`}>{title} is not available yet.</NotYet>
      </Tabs>
    </Page>
  );
}
