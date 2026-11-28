// Contacts (DASHPAY §4, DP2-07): the main identity's contacts in the engine's groups, a sort, the
// local and network search, the pending-setup hint, and a detail slot beside the list that
// DP2-08 (Add Contact) and DP2-09 (the contact's pane) fill. DP2-09 adds the Enable DashPay banner
// and a request row's Accept and Ignore. Copy follows the mobile wallets.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ContactSortName } from "../../api/generated/ContactSortName";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { errorMessage } from "../../api/backend";
import { useBackend, useLockStatus } from "../../api/queries";
import { navigate, ui, useUi, type AddContactIntent } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { DashButton, Icon, VisuallyHidden } from "../../components/basics";
import { contactsDisplay, contactsHeaderAdd, EMPTY } from "../../components/emptyStates";
import { EmptyState, focusWhenIdle, LoadingState } from "../../components/history";
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
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";
import { ContactGroups, NetworkResults } from "./ContactList";
import { AddContact } from "./AddContact";
import { ContactPane } from "./ContactPane";
import { dropDetailFocusUnless, useActionToast, useRefocus } from "./afterAction";
import { RequestButtons } from "./RequestButtons";

/** Opens Add contact's slot (D12), in place of a selection or a link's card. */
const openAdd = (addContact: AddContactIntent) => ui.set({ addContact, contact: null, contactPrefill: null });

export function ContactsPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const t = useT();
  const addOpen = useUi((s) => s.addContact?.walletId === walletId);
  // A `dashpay://user` link shows the same Add contact section, though the button stays enabled.
  const linkOpen = useUi((s) => s.contactPrefill !== null);
  const { owner, retry } = useContactsOwner(walletId);
  // The empty card that carries Add contact is the only reason to hide the header's button.
  const [emptyCard, setEmptyCard] = useState(false);
  // Opening Contacts asks for a DashPay pass soon (D9); a refused ask leaves the engine's schedule.
  useEffect(() => {
    backend.dashpaySyncSoon().catch(() => undefined);
  }, [backend]);
  return (
    <main className="page page-contacts" data-testid="page-contacts">
      <div className="page-column">
        <header className="top-intro top-intro-row">
          <h1 className="top-intro-title">{t("contacts.title")}</h1>
          {owner.kind === "identity" && !emptyCard && <HeaderAdd walletId={walletId} addOpen={addOpen} linkOpen={linkOpen} />}
        </header>
        {owner.kind === "identity" ? (
          <ContactsView walletId={walletId} identity={owner.identity} network={network} onEmptyCard={setEmptyCard} />
        ) : (
          <NoContacts walletId={walletId} owner={owner} retry={retry} />
        )}
      </div>
    </main>
  );
}

/** The header's Add contact. The page hides it only while the empty card shows the same action. */
function HeaderAdd({ walletId, addOpen, linkOpen }: { walletId: string; addOpen: boolean; linkOpen: boolean }) {
  const t = useT();
  return (
    <DashButton
      variant="filledBlue"
      size="small"
      data-testid="contacts-add"
      disabled={addOpen}
      aria-expanded={addOpen || linkOpen}
      why={t("contacts.addOpen")}
      whyId="add-contact-title"
      onClick={() => openAdd({ walletId, query: "", hit: null })}>
      {t("contacts.add")}
    </DashButton>
  );
}

/** Why the page lists no one (D2): no identity yet, one being registered, DashPay starting, or
 * its status unreadable. */
function NoContacts({ walletId, owner, retry }: { walletId: string; owner: Exclude<ContactsOwner, { kind: "identity" }>; retry: () => void }) {
  const prefill = useUi((s) => s.contactPrefill);
  const t = useT();
  let body;
  switch (owner.kind) {
    case "starting":
      body = <LoadingState testId="contacts-starting" text={t("contacts.starting")} />;
      break;
    case "error":
      body = (
        <EmptyState
          testId="contacts-status-error"
          title={t("contacts.statusError")}
          action={
            <DashButton variant="plainBlue" onClick={retry}>
              {t("contacts.tryAgain")}
            </DashButton>
          }
        />
      );
      break;
    case "registering":
      body = <EmptyState testId="contacts-registering" title={t("contacts.registeringTitle")} message={t("contacts.registeringMessage")} />;
      break;
    case "join":
      body = (
        <EmptyState
          testId="contacts-join"
          art={EMPTY.join.art}
          title={t("contacts.joinDashPay")}
          message={t(`contacts.join.${owner.reason ?? "none"}`)}
          action={
            owner.reason === null && (
              <DashButton variant="filledBlue" data-testid="contacts-join-button" onClick={() => navigate("joinDashPay")}>
                {t("contacts.joinDashPay")}
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

function ContactsView({ walletId, identity, network, onEmptyCard }: { walletId: string; identity: string; network: NetworkInfo; onEmptyCard: (shown: boolean) => void }) {
  const t = useT();
  const [search, setSearch] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const [hiddenOpen, setHiddenOpen] = useState(false);
  const sort = useUi((s) => s.contactSort);
  const selectedId = useUi((s) => s.contact);
  // A pending Show's focus is the selected contact's only: another selection or leaving drops it.
  useEffect(() => dropDetailFocusUnless(selectedId), [selectedId]);
  useEffect(() => () => dropDetailFocusUnless(null), []);
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
  const display = contactsDisplay({
    pending: list.isPending,
    failed: list.isError,
    hasSnapshot: list.data !== undefined,
    rowCount: groups.length,
    updating,
    searching: text !== "",
  });
  useLayoutEffect(() => {
    onEmptyCard(!contactsHeaderAdd(display));
    return () => onEmptyCard(false);
  }, [display.kind, onEmptyCard]);
  const select = (contact: string) => ui.set({ contact, addContact: null, contactPrefill: null });
  const toast = useActionToast(walletId);
  const requests = groups.find((g) => g.section === "requests")?.contacts ?? [];
  const pageHeading = () => document.querySelector<HTMLElement>(".app-body > main h1");
  const arm = useRefocus(
    (contact) => requests.some((c) => c.contact === contact),
    (i) => document.querySelectorAll<HTMLElement>(".contact-row-item.has-actions > .contact-row")[i] ?? pageHeading(),
  );
  const rowDone = (c: ContactSummaryInfo) => (o: ActionOutcome | null) => {
    toast(o, false);
    if (o?.ok) arm(c.contact, requests.indexOf(c));
  };
  const armEnable = useRefocus(() => banner !== null, pageHeading);
  const enableDone = (o: ActionOutcome | null) => {
    toast(o, false);
    if (o?.ok) armEnable("enable");
  };

  let body;
  if (display.kind === "arriving" && list.isPending) body = <LoadingState testId="contacts-loading" text={t("contacts.loading")} />;
  else if (display.kind === "failed")
    body = (
      <EmptyState
        testId="contacts-error"
        title={t("contacts.loadFailed")}
        message={errorMessage(list.error)}
        action={
          <DashButton variant="plainBlue" onClick={() => void list.refetch()}>
            {t("contacts.tryAgain")}
          </DashButton>
        }
      />
    );
  else if (display.kind === "arriving") body = <LoadingState testId="contacts-updating" text={t("contacts.updating")} />;
  else if (display.kind === "search")
    body = (
      <EmptyState
        testId="contacts-no-match"
        art="search"
        title={t("contacts.noMatch", { text })}
        message={t("contacts.noMatchHint")}
        action={
          <DashButton
            variant="filledBlue"
            data-testid="contacts-no-match-clear"
            onClick={() => {
              setSearch("");
              focusWhenIdle(searchRef);
            }}
          >
            {t("contacts.clearSearch")}
          </DashButton>
        }
      />
    );
  else if (display.kind === "empty")
    body = (
      <EmptyState
        testId="contacts-empty"
        art={EMPTY.contacts.art}
        title={EMPTY.contacts.title}
        message={EMPTY.contacts.message}
        action={
          <DashButton variant="filledBlue" data-testid={EMPTY.contacts.action.testId} onClick={() => openAdd({ walletId, query: "", hit: null })}>
            {EMPTY.contacts.action.label}
          </DashButton>
        }
      />
    );
  else
    body = (
      <>
        {updating && (
          <p className="contacts-updating" role="status" data-testid="contacts-updating-line">
            {t("contacts.updating")}
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

  const noContacts = display.kind === "empty";
  const solo = noContacts && !prefill && !add;
  return (
    <>
      <div className="tx-filter-bar">
        <label className="search-field" data-testid="contacts-search">
          <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
          <input
            ref={searchRef}
            type="search"
            value={search}
            placeholder={t("contacts.search")}
            aria-label={t("contacts.search")}
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
          <span>{t("contacts.sortBy")}</span>
          <select data-testid="contacts-sort" value={sort} onChange={(e) => ui.set({ contactSort: e.target.value as ContactSortName })}>
            {SORTS.map((s) => (
              <option key={s.id} value={s.id}>
                {s.title}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="contacts-feedback">
        {banner && <EnableDashPay banner={banner} actions={actions} onDone={enableDone} />}
        {hint && (
          <div className={`contacts-hint card-l1 is-${hint.kind}`} role="status" data-testid="contacts-setup-hint">
            <span>{hint.text}</span>
            {hint.kind === "unlock" && (
              <DashButton variant="tintedBlue" size="small" data-testid="contacts-setup-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
                {t("contacts.unlock")}
              </DashButton>
            )}
          </div>
        )}
      </div>
      <div className={solo ? "contacts-split is-solo" : "contacts-split"} data-testid="contacts-split">
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
        {!solo && (
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
        )}
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
  const t = useT();
  return (
    <div className="contacts-hint contacts-enable card-l1" data-testid="contacts-enable-banner">
      <span className="contacts-enable-text">
        <strong>{t("contacts.enable.title")}</strong>
        <span>
          {banner.fee === null
            ? t("contacts.enable.text")
            : t.rich("contacts.enable.textWithFee", { fee: banner.fee, amount: (text) => <span data-testid="contacts-enable-fee">{text}</span> })}
        </span>
      </span>
      <DashButton variant="tintedBlue" size="small" data-testid="contacts-enable" disabled={actions.busy} onClick={() => void actions.enable().then(onDone)}>
        {actions.isRunning("enable", null) ? t("contacts.enable.busy") : t("contacts.enable.button")}
      </DashButton>
    </div>
  );
}
