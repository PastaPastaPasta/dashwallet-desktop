// Contacts (DASHPAY §4, DP2-07): the main identity's contacts in the engine's groups, a sort, the
// local and network search, the pending-setup hint, and a detail slot beside the list that
// DP2-08 (Add contact) and DP2-09 (the contact's actions) fill. Copy follows the mobile wallets.

import { useEffect, useMemo, useRef, useState } from "react";
import type { ContactSortName } from "../../api/generated/ContactSortName";
import { errorMessage } from "../../api/backend";
import { useBackend, useLockStatus } from "../../api/queries";
import { navigate, ui, useUi, type AddContactIntent } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton, Icon } from "../../components/basics";
import { EmptyState, LoadingState } from "../../components/history";
import {
  findContact,
  groups as toGroups,
  listedIds,
  NETWORK_DEBOUNCE_MS,
  networkHits,
  networkPrefix,
  SEARCH_DEBOUNCE_MS,
  setupHint,
  SORTS,
  type ContactsOwner,
} from "../../contacts/model";
import { useContactList, useContactsOwner, useNetworkSearch, usePendingSetup } from "../../contacts/queries";
import { ContactGroups, NetworkResults } from "./ContactList";
import { ContactPane } from "./ContactPane";

const openAdd = (addContact: AddContactIntent) => ui.set({ addContact, contact: null });

export function ContactsPage({ walletId }: { walletId: string }) {
  const backend = useBackend();
  const { owner } = useContactsOwner(walletId);
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
            <DashButton variant="filledBlue" size="small" data-testid="contacts-add" onClick={() => openAdd({ query: "", hit: null })}>
              Add contact
            </DashButton>
          )}
        </header>
        {owner.kind === "identity" ? <ContactsView walletId={walletId} identity={owner.identity} /> : <NoContacts owner={owner} />}
      </div>
    </main>
  );
}

/** Why the page lists no one (D2): no identity yet, one being registered, or DashPay starting. */
function NoContacts({ owner }: { owner: Exclude<ContactsOwner, { kind: "identity" }> }) {
  const prefill = useUi((s) => s.contactPrefill);
  const body =
    owner.kind === "starting" ? (
      <LoadingState testId="contacts-starting" text="Starting DashPay…" />
    ) : owner.kind === "registering" ? (
      <EmptyState testId="contacts-registering" title="Your username is being registered" message="Your contacts show here once Dash Platform confirms it." />
    ) : (
      <EmptyState
        testId="contacts-join"
        title="Join DashPay"
        message={
          owner.reason === "watchOnly"
            ? "A watch-only wallet cannot join DashPay."
            : owner.reason === "waitingForSync"
              ? "You can join DashPay once the wallet has synced."
              : "Create a username to add contacts and pay them by name."
        }
        action={
          owner.reason === null && (
            <DashButton variant="filledBlue" data-testid="contacts-join-button" onClick={() => navigate("joinDashPay")}>
              Join DashPay
            </DashButton>
          )
        }
      />
    );
  // A `dashpay://user` link still shows what it carried (U-01 D12).
  return (
    <>
      {prefill && <ContactPane prefill={prefill} add={null} selected={null} />}
      {body}
    </>
  );
}

function ContactsView({ walletId, identity }: { walletId: string; identity: string }) {
  const [search, setSearch] = useState("");
  const [hiddenOpen, setHiddenOpen] = useState(false);
  const sort = useUi((s) => s.contactSort);
  const selectedId = useUi((s) => s.contact);
  const add = useUi((s) => s.addContact);
  const prefill = useUi((s) => s.contactPrefill);
  const text = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  // The network prefix waits longer; clearing the field hides its rows at once (D6).
  const prefix = useDebounced(networkPrefix(search), NETWORK_DEBOUNCE_MS);
  const typedPrefix = networkPrefix(search);

  const list = useContactList(walletId, identity, sort, text);
  const pending = usePendingSetup(walletId);
  const lock = useLockStatus();
  const net = useNetworkSearch(walletId, prefix);

  const groups = toGroups(list.data, { hiddenOpen, searching: text !== "" });
  // A search that filters the selected contact out keeps it in the detail slot (D11).
  const found = findContact(list.data, selectedId);
  const lastFound = useRef(found);
  if (found) lastFound.current = found;
  const selected = found ?? (text !== "" && lastFound.current?.contact.contact === selectedId ? lastFound.current : null);
  const hint = setupHint(pending.data, lock.data?.state);
  const listed = useMemo(() => listedIds(list.data), [list.data]);
  const hits = net.data ? networkHits(net.data, listed) : { shown: [], more: false };
  const updating = list.data?.updating === true;
  const select = (contact: string) => ui.set({ contact, addContact: null, contactPrefill: null });

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
          <DashButton variant="filledBlue" data-testid="contacts-empty-search" onClick={() => openAdd({ query: "", hit: null })}>
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
        <ContactGroups groups={groups} selected={selected?.contact.contact ?? null} onSelect={select} onToggleHidden={() => setHiddenOpen((o) => !o)} />
      </>
    );

  return (
    <>
      <div className="contacts-toolbar">
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
              state={net.isPending ? "loading" : net.isError ? "error" : "done"}
              shown={hits.shown}
              more={hits.more}
              onRetry={() => void net.refetch()}
              onPick={(h) => openAdd({ query: prefix, hit: { identity: h.identity, username: h.username } })}
              onMore={() => openAdd({ query: prefix, hit: null })}
            />
          )}
        </div>
        <ContactPane prefill={prefill} add={add} selected={selected} />
      </div>
    </>
  );
}
