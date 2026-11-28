// The bell (DASHPAY §4 `NotificationsViewModel`, DP2-09 D11–D13): its popover from the toolbar and
// the Notifications page. Both list the contact requests waiting, with Accept and Ignore, then the
// identity's events: New (unread when the view opened) and, on the page, Earlier.

import { useEffect, useId, useRef, type RefObject } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { DashPayEventInfo } from "../../api/generated/DashPayEventInfo";
import { errorMessage } from "../../api/backend";
import { useBackend } from "../../api/queries";
import { navigate, ui, useUi } from "../../app/ui";
import { Chip, DashButton } from "../../components/basics";
import { EmptyArt } from "../../components/emptyArt";
import { EMPTY } from "../../components/emptyStates";
import { EmptyState, LoadingState } from "../../components/history";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { avatarLetters, initials, shownName, subtitle } from "../../contacts/model";
import { useContactActions, useContactList, useContactsOwner, useHasKeys } from "../../contacts/queries";
import { detailDate } from "../../format/tx";
import { useT } from "../../i18n";
import { eventLink, eventText, feedOf, namesFrom, popoverCut, type NameOf } from "../../notifications/model";
import { useEventFeed, useReadOnOpen } from "../../notifications/queries";
import { useReceived } from "../../payments/received";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { useActionToast, useRefocus } from "../contacts/afterAction";
import { RequestButtons } from "../contacts/RequestButtons";
import { Page } from "../Routes";
import { ReceivedSection } from "./Received";
import { GrantSheet } from "../tools/common";

/** What a view of the bell reads: the feed, read on opening, the names its events use, and the
 * actions its request rows run (one model per view, D16). */
function useBell(walletId: string, identity: string) {
  const sort = useUi((s) => s.contactSort);
  const feed = useEventFeed(walletId, identity);
  const unreadAtOpen = useReadOnOpen(walletId, identity, feed);
  const list = useContactList(walletId, identity, sort, "");
  const actions = useContactActions(walletId, identity);
  const hasKeys = useHasKeys(walletId, identity);
  return { feed, shown: feedOf(feed.data?.pages ?? [], unreadAtOpen), nameOf: namesFrom(list.data), actions, hasKeys };
}

/** What a request row's action calls when it ends: the row's contact and place among the rows. */
type RequestDone = (outcome: ActionOutcome | null, contact: string, index: number) => void;

/** A view's request rows after an action (D23, D28): the outcome is said inline while the view
 * is open, else in a toast; a request accepted or ignored leaves, and focus goes to the request
 * now in its place, else to `heading`. */
function useRequestDone(walletId: string, pending: ContactSummaryInfo[], root: RefObject<HTMLElement | null>, heading: () => HTMLElement | null | undefined): RequestDone {
  const toast = useActionToast(walletId);
  const arm = useRefocus(
    (contact) => pending.some((c) => c.contact === contact),
    (i) => root.current?.querySelectorAll<HTMLElement>(".bell-row.has-actions > .bell-row-open")[i] ?? heading(),
  );
  return (o, contact, index) => {
    toast(o, false);
    if (o?.ok) arm(contact, index);
  };
}

/** Opens `contact`'s pane in Contacts (D12). */
const openContact = (contact: string) => navigate("contacts", { contact, addContact: null, contactPrefill: null });

/** Opens what an event is about (D12): the contact's pane, or the payment's transaction. */
function openEvent(e: DashPayEventInfo) {
  const link = eventLink(e);
  if (link?.kind === "contact") openContact(link.contact);
  else if (link?.kind === "tx") ui.set({ detail: link.txid });
}

/** The popover under the bell (D11). Escape or a click outside closes it, except while an action
 * or its passphrase prompt is open, and so does focus moving out of it; `close` gives focus back to
 * the bell unless told not to. */
export function NotificationsPopover({
  walletId,
  identity,
  network,
  id,
  close,
}: {
  walletId: string;
  identity: string;
  network: NetworkInfo;
  id: string;
  close: (o?: { refocus?: boolean }) => void;
}) {
  const t = useT();
  const { feed, shown, nameOf, actions, hasKeys } = useBell(walletId, identity);
  useModel(actions);
  const root = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const busy = actions.busy;
  const cut = popoverCut(shown);
  const onDone = useRequestDone(walletId, cut.pending, root, () => heading.current);
  useEffect(() => heading.current?.focus(), []);
  useEffect(() => {
    if (busy) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      close();
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as Element | null;
      // The bell toggles on its own click.
      if (!root.current?.contains(t) && !t?.closest?.("[data-bell]")) close();
    };
    const left = (t: Element | null) => !!t && t !== document.body && !root.current?.contains(t) && !t.closest?.('[data-bell], [aria-modal="true"]');
    const onFocus = (e: FocusEvent) => {
      if (left(e.target as Element | null)) close({ refocus: false });
    };
    // Focus may have left while an action ran.
    if (left(document.activeElement)) return close({ refocus: false });
    document.addEventListener("keydown", onKey);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("focusin", onFocus);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("focusin", onFocus);
    };
  }, [busy, close]);
  const go = (then: () => void) => {
    close();
    then();
  };
  const payments = useReceived(walletId);
  const empty = feed.isSuccess && cut.pending.length === 0 && cut.fresh.length === 0 && payments.length === 0;
  return (
    <div className="bell-popover card-l1" id={id} role="dialog" aria-labelledby={`${id}-title`} ref={root} data-testid="bell-popover">
      <h2 className="bell-title" id={`${id}-title`} tabIndex={-1} ref={heading}>
        {t("notifications.title")}
      </h2>
      <div className="bell-content">
        {feed.isPending && <p className="placeholder-note">{t("notifications.loading")}</p>}
        {feed.isError && (
          <p className="form-problem" role="alert">
            {t("notifications.readErrorDetail", { error: errorMessage(feed.error) })}
          </p>
        )}
        {empty && (
          <div className="bell-empty" data-testid="bell-empty">
            <EmptyArt id={EMPTY.notifications.art} size={40} />
            <p>{t("notifications.emptyPopover")}</p>
          </div>
        )}
        <ReceivedSection walletId={walletId} network={network} limit={5} onOpen={(txid) => go(() => ui.set({ detail: txid }))} />
        {cut.pending.length > 0 && (
          <Requests contacts={cut.pending} more={cut.morePending} actions={actions} hasKeys={hasKeys} onOpen={(c) => go(() => openContact(c))} onDone={onDone} testId="bell-requests" />
        )}
        {cut.fresh.length > 0 && <Events title={t("notifications.section.new")} events={cut.fresh} fresh more={cut.moreFresh} nameOf={nameOf} onOpen={(e) => go(() => openEvent(e))} testId="bell-new" />}
      </div>
      <DashButton variant="plainBlue" size="small" className="bell-all" data-testid="bell-see-all" onClick={() => go(() => navigate("notifications"))}>
        {t("notifications.seeAll")}
      </DashButton>
      <GrantSheet grants={actions.grants} />
    </div>
  );
}

/** The Notifications page (D12): every request waiting and every event, a page at a time. */
export function NotificationsPage({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const t = useT();
  const { owner } = useContactsOwner(walletId);
  const received = useReceived(walletId);
  let body;
  // Another main identity starts the view over: its own feed, read and marked afresh.
  if (owner.kind === "identity") body = <NotificationsView key={owner.identity} walletId={walletId} identity={owner.identity} />;
  else if (owner.kind === "starting") body = <LoadingState testId="notifications-starting" text={t("notifications.startingDashPay")} />;
  else if (received.length === 0)
    body = (
      <EmptyState
        testId="notifications-no-identity"
        art={EMPTY.notificationsNoIdentity.art}
        title={EMPTY.notificationsNoIdentity.title}
        message={EMPTY.notificationsNoIdentity.message}
        action={
          owner.kind === "join" && owner.reason === null && (
            <DashButton variant="filledBlue" data-testid={EMPTY.notificationsNoIdentity.action.testId} onClick={() => navigate("joinDashPay")}>
              {EMPTY.notificationsNoIdentity.action.label}
            </DashButton>
          )
        }
      />
    );
  else body = null;
  return (
    <Page route="notifications">
      <div className="notifications" data-testid="notifications-payments">
        <ReceivedSection walletId={walletId} network={network} onOpen={(txid) => ui.set({ detail: txid })} />
      </div>
      {body}
    </Page>
  );
}

function NotificationsView({ walletId, identity }: { walletId: string; identity: string }) {
  const t = useT();
  const { feed, shown, nameOf, actions, hasKeys } = useBell(walletId, identity);
  useModel(actions);
  const root = useRef<HTMLDivElement>(null);
  const onDone = useRequestDone(walletId, shown.pending, root, () => root.current?.closest("main")?.querySelector("h1"));
  const hasPayments = useReceived(walletId).length > 0;
  if (feed.isPending) return <LoadingState testId="notifications-loading" text={t("notifications.loadingPage")} />;
  if (feed.isError && !feed.data)
    return (
      <EmptyState
        testId="notifications-error"
        title={t("notifications.readError")}
        message={errorMessage(feed.error)}
        action={
          <DashButton variant="plainBlue" onClick={() => void feed.refetch()}>
            {t("notifications.tryAgain")}
          </DashButton>
        }
      />
    );
  return (
    <div className="notifications" data-testid="notifications" ref={root}>
      {!hasPayments && shown.pending.length === 0 && shown.fresh.length === 0 && shown.earlier.length === 0 && EMPTY.notifications.action && (
        <EmptyState
          testId="notifications-empty"
          art={EMPTY.notifications.art}
          title={EMPTY.notifications.title}
          message={EMPTY.notifications.message}
          action={
            <DashButton
              variant="filledBlue"
              data-testid={EMPTY.notifications.action.testId}
              onClick={() => navigate("contacts", { addContact: { walletId, query: "", hit: null }, contact: null, contactPrefill: null })}
            >
              {EMPTY.notifications.action.label}
            </DashButton>
          }
        />
      )}
      {shown.pending.length > 0 && <Requests contacts={shown.pending} more={0} actions={actions} hasKeys={hasKeys} onOpen={openContact} onDone={onDone} testId="notifications-requests" />}
      {shown.fresh.length > 0 && <Events title={t("notifications.section.new")} events={shown.fresh} fresh more={0} nameOf={nameOf} onOpen={openEvent} testId="notifications-new" />}
      {shown.earlier.length > 0 && <Events title={t("notifications.section.earlier")} events={shown.earlier} fresh={false} more={0} nameOf={nameOf} onOpen={openEvent} testId="notifications-earlier" />}
      {feed.hasNextPage && (
        <DashButton variant="plainBlue" size="small" data-testid="notifications-more" disabled={feed.isFetchingNextPage} onClick={() => void feed.fetchNextPage()}>
          {feed.isFetchingNextPage ? t("notifications.loading") : t("notifications.showMore")}
        </DashButton>
      )}
      <GrantSheet grants={actions.grants} />
    </div>
  );
}

/** "Contact requests": each with Accept and Ignore beside the row that opens its pane (D15). */
function Requests({
  contacts,
  more,
  actions,
  hasKeys,
  onOpen,
  onDone,
  testId,
}: {
  contacts: ContactSummaryInfo[];
  more: number;
  actions: ContactActions;
  hasKeys: boolean | undefined;
  onOpen: (contact: string) => void;
  onDone: RequestDone;
  testId: string;
}) {
  const t = useT();
  const titleId = useId();
  return (
    <section className="bell-section" aria-labelledby={titleId} data-testid={testId}>
      <h3 className="bell-section-title" id={titleId}>
        {t("notifications.section.requests")}
      </h3>
      <ul className="bell-rows">
        {contacts.map((c, i) => {
          const sub = subtitle(c);
          return (
            <li key={c.contact} className="bell-row has-actions" data-testid="bell-request" data-contact={c.contact}>
              <button type="button" className="bell-row-open" onClick={() => onOpen(c.contact)}>
                <Avatar letters={avatarLetters(c)} identity={c.contact} />
                <span className="bell-row-text">
                  <span className="bell-row-title">
                    <bdi>{shownName(c)}</bdi>
                    {c.unverified && <Chip tone="warning">{t("contacts.unverified")}</Chip>}
                  </span>
                  {sub && (
                    <span className="bell-row-sub">
                      <bdi>{sub}</bdi>
                    </span>
                  )}
                </span>
              </button>
              <RequestButtons contact={c} actions={actions} hasKeys={hasKeys} testPrefix="bell" onDone={(o) => onDone(o, c.contact, i)} />
            </li>
          );
        })}
      </ul>
      {more > 0 && <p className="bell-more">{t("notifications.moreRequests", { count: more })}</p>}
      {hasKeys === false && (
        <p className="bell-more" data-testid={`${testId}-enable-first`}>
          {t("notifications.enableFirst")}
        </p>
      )}
    </section>
  );
}

/** A list of events (D13); a row with somewhere to go is a button (D12). */
function Events({
  title,
  events,
  fresh,
  more,
  nameOf,
  onOpen,
  testId,
}: {
  title: string;
  events: DashPayEventInfo[];
  fresh: boolean;
  more: number;
  nameOf: NameOf;
  onOpen: (e: DashPayEventInfo) => void;
  testId: string;
}) {
  const t = useT();
  const titleId = useId();
  const { locale } = useBackend();
  return (
    <section className="bell-section" aria-labelledby={titleId} data-testid={testId}>
      <h3 className="bell-section-title" id={titleId}>
        {title}
      </h3>
      <ul className="bell-rows">
        {events.map((e) => {
          const body = (
            <>
              {fresh && <span className="bell-dot" aria-hidden="true" />}
              {e.contact && <Avatar letters={initials(nameOf(e.contact))} identity={e.contact} />}
              <span className="bell-row-text">
                <span className="bell-row-title">{eventText(e, nameOf)}</span>
                <span className="bell-row-sub">{detailDate(new Date(e.at * 1000), locale)}</span>
              </span>
            </>
          );
          return (
            <li key={e.id} className={fresh ? "bell-row is-new" : "bell-row"} data-testid="bell-event" data-kind={e.kind}>
              {eventLink(e) ? (
                <button type="button" className="bell-row-open" onClick={() => onOpen(e)}>
                  {body}
                </button>
              ) : (
                <span className="bell-row-open">{body}</span>
              )}
            </li>
          );
        })}
      </ul>
      {more > 0 && <p className="bell-more">{t("notifications.moreEvents", { count: more })}</p>}
    </section>
  );
}
