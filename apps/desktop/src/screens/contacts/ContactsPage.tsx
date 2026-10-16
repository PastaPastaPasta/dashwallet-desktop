// Contacts (DASHPAY §4, DP2-07): the main identity's contacts in the engine's groups, a sort, the
// local and network search, the pending-setup hint, and a detail slot beside the list that
// DP2-08 (Add Contact) and DP2-09 (the contact's pane) fill. DP2-09 adds the Enable DashPay banner
// and a request row's Accept and Ignore. Copy follows the mobile wallets.

import { useEffect, useRef, useState } from "react";
import type { ContactSortName } from "../../api/generated/ContactSortName";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { errorMessage } from "../../api/backend";
import { useBackend, useLockStatus } from "../../api/queries";
import { navigate, ui, useUi, type AddContactIntent } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton, Icon, VisuallyHidden } from "../../components/basics";
import { EmptyState, LoadingState } from "../../components/history";
import {
  findContact,
  groups as toGroups,
  NETWORK_DEBOUNCE_MS,
  networkHits,
  networkPrefix,
  SEARCH_DEBOUNCE_MS,
  setupHint,
  SORTS,
  type ContactsOwner,
} from "../../contacts/model";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { listedNow } from "../../contacts/add";
import { selectionAnnouncement, type EnableBanner } from "../../contacts/pane";
import { useContactActions, useContactList, useContactsOwner, useEnableBanner, useNetworkSearch, usePendingSetup } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { GrantSheet, Outcome } from "../tools/common";
import { ContactGroups, NetworkResults } from "./ContactList";
import { AddContact } from "./AddContact";
import { ContactPane } from "./ContactPane";
import { useActionToast, useRefocus } from "./afterAction";
import { RequestButtons } from "./RequestButtons";

/** Opens Add contact's slot (D12), in place of a selection or a link's card. */
const openAdd = (addContact: AddContactIntent) => ui.set({ addContact, contact: null, contactPrefill: null });

export function ContactsPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const { owner, retry } = useContactsOwner(walletId);
  // Opening Contacts asks for a DashPay pass soon (D9); a refused ask leaves the engine's schedule.
  useEffect(() => {
    backend.dashpaySyncSoon().catch(() => undefined);
  }, [backend]);
  return (
    <main className="page page-contacts" data-testid="page-contacts">
      <div className="page-column">
        <header className="top-intro top-intro-row">
          <h1 className="top-intro-title">Contacts</h1>
          {owner.kind === "identity" && (
            <DashButton variant="filledBlue" size="small" data-testid="contacts-add" onClick={() => openAdd({ walletId, query: "", hit: null })}>
              Add contact
            </DashButton>
          )}
        </header>
        {owner.kind === "identity" ? (
          <ContactsView walletId={walletId} identity={owner.identity} network={network} />
        ) : (
          <NoContacts walletId={walletId} owner={owner} retry={retry} />
        )}
      </div>
    </main>
  );
}

const JOIN_MESSAGES = {
  watchOnly: "A watch-only wallet cannot join DashPay.",
  waitingForSync: "You can join DashPay once the wallet has synced.",
  none: "Create a username to add contacts and pay them by name.",
};

/** Why the page lists no one (D2): no identity yet, one being registered, DashPay starting, or
 * its status unreadable. */
function NoContacts({ walletId, owner, retry }: { walletId: string; owner: Exclude<ContactsOwner, { kind: "identity" }>; retry: () => void }) {
  const prefill = useUi((s) => s.contactPrefill);
  let body;
  switch (owner.kind) {
    case "starting":
      body = <LoadingState testId="contacts-starting" text="Starting DashPay…" />;
      break;
    case "error":
      body = (
        <EmptyState
          testId="contacts-status-error"
          title="Could not read DashPay's state"
          action={
            <DashButton variant="plainBlue" onClick={retry}>
              Try again
            </DashButton>
          }
        />
      );
      break;
    case "registering":
      body = <EmptyState testId="contacts-registering" title="Your username is being registered" message="Your contacts show here once Dash Platform confirms it." />;
      break;
    case "join":
      body = (
        <EmptyState
          testId="contacts-join"
          title="Join DashPay"
          message={JOIN_MESSAGES[owner.reason ?? "none"]}
          action={
            owner.reason === null && (
              <DashButton variant="filledBlue" data-testid="contacts-join-button" onClick={() => navigate("joinDashPay")}>
                Join DashPay
              </DashButton>
            )
          }
        />
      );
  }
  // A `dashpay://user` link still shows who it leads to, verified, with what the wallet can do
  // (DP2-08 D13).
  return (
    <>
      {prefill && (
        <ContactPane
          adding={<AddContact key={walletId} ctx={{ walletId, owner, actions: null, listed: undefined }} intent={null} link={prefill} />}
          selected={null}
          pane={null}
        />
      )}
      {body}
    </>
  );
}

function ContactsView({ walletId, identity, network }: { walletId: string; identity: string; network: NetworkInfo }) {
  const [search, setSearch] = useState("");
  const [hiddenOpen, setHiddenOpen] = useState(false);
  const sort = useUi((s) => s.contactSort);
  const selectedId = useUi((s) => s.contact);
  const intent = useUi((s) => s.addContact);
  const prefill = useUi((s) => s.contactPrefill);
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  // The network prefix waits longer; clearing the field hides its rows at once (D6).
  const prefix = useDebounced(networkPrefix(search), NETWORK_DEBOUNCE_MS);
  const typedPrefix = networkPrefix(search);

  const list = useContactList(walletId, identity, sort, text);
  // Everyone this identity relates to, unfiltered (the same query as `list` without a search): the
  // selection and the network rows are checked against it (D11, D21).
  const all = useContactList(walletId, identity, sort, "");
  const pending = usePendingSetup(walletId);
  const lock = useLockStatus();
  const net = useNetworkSearch(walletId, identity, prefix);
  // One actions model and passphrase prompt for the pane, the rows and the banner (DP2-09 D16).
  const actions = useContactActions(walletId, identity);
  const { banner, hasKeys } = useEnableBanner(walletId, identity);

  // Another wallet's Add intent is not this one's (D12).
  const add = intent?.walletId === walletId ? intent : null;
  // Searching is what the shown rows were read for: a new sort or search loading over them keeps
  // the earlier answer's layout (D5).
  const shownText = useRef(text);
  if (!list.isPlaceholderData) shownText.current = text;
  const groups = toGroups(list.data, { hiddenOpen, searching: shownText.current !== "" });
  // A search that filters the selected contact out keeps it in the detail slot, as long as this
  // identity still relates to it (D11).
  const selected = findContact(list.data, selectedId) ?? (text !== "" ? findContact(all.data, selectedId) : null);
  const hint = setupHint(pending.data, lock.data?.state);
  const hits = networkHits(net.data, all.data);
  const updating = list.data?.updating === true;
  const select = (contact: string) => ui.set({ contact, addContact: null, contactPrefill: null });
  // A row's Accept or Ignore: the pane says its own contact's outcome, a toast another's; focus
  // goes to the request now in the row's place, else to the page's heading (D28).
  const toast = useActionToast(walletId);
  const requests = groups.find((g) => g.section === "requests")?.contacts ?? [];
  const pageHeading = () => document.querySelector<HTMLElement>(".app-body > main h1");
  const arm = useRefocus(
    (contact) => requests.some((c) => c.contact === contact),
    (i) => document.querySelectorAll<HTMLElement>(".contact-row-item.has-actions > .contact-row")[i] ?? pageHeading(),
  );
  // The contact whose pane shows now (a link's card or Add contact take the slot), read when an
  // action ends.
  const paneShows = useRef<string | null>(null);
  paneShows.current = prefill || add ? null : (selected?.contact ?? null);
  const rowDone = (c: ContactSummaryInfo) => (o: ActionOutcome | null) => {
    toast(o, paneShows.current === c.contact);
    if (o?.ok) arm(c.contact, requests.indexOf(c));
  };
  // Enable: success removes the banner, so a toast says it and focus goes to the heading.
  const armEnable = useRefocus(() => banner !== null, pageHeading);
  const enableDone = (o: ActionOutcome | null) => {
    toast(o, !o?.ok);
    if (o?.ok) armEnable("enable");
  };

  let body;
  if (list.isPending) body = <LoadingState testId="contacts-loading" text="Loading contacts…" />;
  else if (list.isError && !list.data)
    body = (
      <EmptyState
        testId="contacts-error"
        title="Could not load your contacts"
        message={errorMessage(list.error)}
        action={
          <DashButton variant="plainBlue" onClick={() => void list.refetch()}>
            Try again
          </DashButton>
        }
      />
    );
  else if (groups.length === 0 && updating) body = <LoadingState testId="contacts-updating" text="Updating contacts…" />;
  else if (groups.length === 0 && text !== "") body = <EmptyState testId="contacts-no-match" title={`There are no contacts that match “${text}”`} />;
  else if (groups.length === 0)
    body = (
      <EmptyState
        testId="contacts-empty"
        title="No contacts yet"
        message="Search for a user by their username to send them a contact request."
        action={
          <DashButton variant="filledBlue" data-testid="contacts-empty-search" onClick={() => openAdd({ walletId, query: "", hit: null })}>
            Search for a User
          </DashButton>
        }
      />
    );
  else
    body = (
      <>
        {updating && (
          <p className="contacts-updating" role="status" data-testid="contacts-updating-line">
            Updating contacts…
          </p>
        )}
        <ContactGroups
          groups={groups}
          selected={selected?.contact ?? null}
          onSelect={select}
          onToggleHidden={() => setHiddenOpen((o) => !o)}
          rowActions={(c) =>
            c.relation === "pendingIncoming" && <RequestButtons contact={c} actions={actions} hasKeys={hasKeys} testPrefix="contact-row" onDone={rowDone(c)} />
          }
        />
      </>
    );

  return (
    <>
      <div className="tx-filter-bar">
        <label className="search-field" data-testid="contacts-search">
          <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
          <input
            type="search"
            value={search}
            placeholder="Search for a contact"
            aria-label="Search for a contact"
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && search !== "") {
                e.preventDefault();
                setSearch("");
              }
            }}
          />
        </label>
        <label className="filter-select">
          <span>Sort by</span>
          <select data-testid="contacts-sort" value={sort} onChange={(e) => ui.set({ contactSort: e.target.value as ContactSortName })}>
            {SORTS.map((s) => (
              <option key={s.id} value={s.id}>
                {s.title}
              </option>
            ))}
          </select>
        </label>
      </div>
      {banner && <EnableDashPay banner={banner} actions={actions} onDone={enableDone} />}
      {hint && (
        <div className={`contacts-hint card-l1 is-${hint.kind}`} role="status" data-testid="contacts-setup-hint">
          <span>{hint.text}</span>
          {hint.kind === "unlock" && (
            <DashButton variant="tintedBlue" size="small" data-testid="contacts-setup-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
              Unlock
            </DashButton>
          )}
        </div>
      )}
      <div className="contacts-split">
        <div className="contacts-list" data-testid="contacts-list">
          {body}
          {prefix !== null && typedPrefix !== null && (
            <NetworkResults
              // Try again shows the search running, not the old error.
              state={net.isPending || (net.isError && net.isFetching) ? "loading" : net.isError ? "error" : "done"}
              shown={hits.shown}
              more={hits.more}
              onRetry={() => void net.refetch()}
              onPick={(h) => openAdd({ walletId, query: prefix, hit: { identity: h.identity, username: h.username } })}
              onMore={() => openAdd({ walletId, query: prefix, hit: null })}
            />
          )}
        </div>
        <ContactPane
          adding={
            (prefill || add) && (
              <AddContact
                key={walletId}
                ctx={{ walletId, owner: { kind: "identity", identity }, actions, listed: listedNow(all.data) }}
                intent={add}
                link={prefill}
              />
            )
          }
          selected={selected}
          pane={{ walletId, identity, network, actions, hasKeys }}
        />
      </div>
      {/* The detail slot's change, said once (DP2-09 D14). */}
      <VisuallyHidden>
        <span role="status" aria-live="polite" data-testid="contacts-announcer">
          {prefill || add ? "" : selectionAnnouncement(selected)}
        </span>
      </VisuallyHidden>
      <GrantSheet grants={actions.grants} />
    </>
  );
}

/** "Enable DashPay" (DP2-09 D10): the main identity lacks its DashPay keys, so it cannot accept
 * requests nor pay contacts until it adds them. Success removes the banner, so a toast says it. */
function EnableDashPay({ banner, actions, onDone }: { banner: EnableBanner; actions: ContactActions; onDone: (o: ActionOutcome | null) => void }) {
  useModel(actions);
  const outcome = actions.outcome?.kind === "enable" && !actions.outcome.ok ? actions.outcome : null;
  return (
    <div className="contacts-hint contacts-enable card-l1" data-testid="contacts-enable-banner">
      <span className="contacts-enable-text">
        <strong>Enable DashPay</strong>
        <span>
          Add the keys DashPay needs to accept contact requests and pay contacts
          {banner.fee !== null && (
            <>
              {" "}
              (a fee of about <span data-testid="contacts-enable-fee">{banner.fee.toLocaleString()}</span> duffs in credits)
            </>
          )}
          .
        </span>
        <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="contacts-enable-outcome" />
      </span>
      <DashButton variant="tintedBlue" size="small" data-testid="contacts-enable" disabled={actions.busy} onClick={() => void actions.enable().then(onDone)}>
        {actions.isRunning("enable", null) ? "Enabling…" : "Enable"}
      </DashButton>
    </div>
  );
}
