// The bell (DASHPAY §4 `NotificationsViewModel`, DP2-09 D11–D13): its popover from the toolbar and
// the Notifications page. Both list the contact requests waiting, with Accept and Ignore, then the
// identity's events: New (unread when the view opened) and, on the page, Earlier.

import { useCallback, useEffect, useId, useRef } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { DashPayEventInfo } from "../../api/generated/DashPayEventInfo";
import { errorMessage } from "../../api/backend";
import { navigate, ui, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { showToast } from "../../components/toast";
import { EmptyState, LoadingState } from "../../components/history";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import { avatarLetters, shownName, subtitle } from "../../contacts/model";
import { ENABLE_FIRST } from "../../contacts/pane";
import { useContactActions, useContactList, useContactsOwner, useHasKeys } from "../../contacts/queries";
import { detailDate } from "../../format/tx";
import { eventLink, eventText, feedOf, namesFrom, popoverCut, type NameOf } from "../../notifications/model";
import { useEventFeed, useReadOnOpen } from "../../notifications/queries";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { RequestButtons } from "../contacts/RequestButtons";
import { Page } from "../Routes";
import { GrantSheet, Outcome } from "../tools/common";

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

/** An action's outcome that lands after its view closed (the popover, the page left): the action
 * ran on, so a toast says how it ended (D4). */
function useToastWhenGone(): (outcome: ActionOutcome | null) => void {
  const gone = useRef(false);
  useEffect(() => {
    gone.current = false;
    return () => {
      gone.current = true;
    };
  }, []);
  return useCallback((o) => {
    if (o && gone.current) showToast(o.text);
  }, []);
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
 * or its passphrase prompt is open; `close` gives focus back to the bell. */
export function NotificationsPopover({ walletId, identity, id, close }: { walletId: string; identity: string; id: string; close: () => void }) {
  const { feed, shown, nameOf, actions, hasKeys } = useBell(walletId, identity);
  useModel(actions);
  const root = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const busy = actions.busy;
  const onDone = useToastWhenGone();
  const cut = popoverCut(shown);
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
    document.addEventListener("keydown", onKey);
    document.addEventListener("mousedown", onDown);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("mousedown", onDown);
    };
  }, [busy, close]);
  const go = (then: () => void) => {
    close();
    then();
  };
  const empty = feed.isSuccess && cut.pending.length === 0 && cut.fresh.length === 0;
  return (
    <div className="bell-popover card-l1" id={id} role="dialog" aria-labelledby={`${id}-title`} ref={root} data-testid="bell-popover">
      <h2 className="bell-title" id={`${id}-title`} tabIndex={-1} ref={heading}>
        Notifications
      </h2>
      {feed.isPending && <p className="placeholder-note">Loading…</p>}
      {feed.isError && (
        <p className="form-problem" role="alert">
          Could not read the notifications: {errorMessage(feed.error)}
        </p>
      )}
      {empty && (
        <p className="placeholder-note" data-testid="bell-empty">
          No new notifications
        </p>
      )}
      {cut.pending.length > 0 && (
        <Requests contacts={cut.pending} more={cut.morePending} actions={actions} hasKeys={hasKeys} onOpen={(c) => go(() => openContact(c))} onDone={onDone} testId="bell-requests" />
      )}
      {cut.fresh.length > 0 && <Events title="New" events={cut.fresh} fresh more={cut.moreFresh} nameOf={nameOf} onOpen={(e) => go(() => openEvent(e))} testId="bell-new" />}
      <Outcome ok={actions.outcome?.ok ?? true} text={actions.outcome?.text ?? null} testId="bell-outcome" />
      <DashButton variant="plainBlue" size="small" className="bell-all" data-testid="bell-see-all" onClick={() => go(() => navigate("notifications"))}>
        See all notifications
      </DashButton>
      <GrantSheet grants={actions.grants} />
    </div>
  );
}

/** The Notifications page (D12): every request waiting and every event, a page at a time. */
export function NotificationsPage({ walletId }: { walletId: string }) {
  const { owner } = useContactsOwner(walletId);
  let body;
  // Another main identity starts the view over: its own feed, read and marked afresh.
  if (owner.kind === "identity") body = <NotificationsView key={owner.identity} walletId={walletId} identity={owner.identity} />;
  else if (owner.kind === "starting") body = <LoadingState testId="notifications-starting" text="Starting DashPay…" />;
  else body = <EmptyState testId="notifications-no-identity" title="No notifications" message="Create a username to get contact requests and payments from contacts." />;
  return <Page route="notifications">{body}</Page>;
}

function NotificationsView({ walletId, identity }: { walletId: string; identity: string }) {
  const { feed, shown, nameOf, actions, hasKeys } = useBell(walletId, identity);
  useModel(actions);
  const onDone = useToastWhenGone();
  if (feed.isPending) return <LoadingState testId="notifications-loading" text="Loading notifications…" />;
  if (feed.isError && !feed.data)
    return (
      <EmptyState
        testId="notifications-error"
        title="Could not read the notifications"
        message={errorMessage(feed.error)}
        action={
          <DashButton variant="plainBlue" onClick={() => void feed.refetch()}>
            Try again
          </DashButton>
        }
      />
    );
  return (
    <div className="notifications" data-testid="notifications">
      <Outcome ok={actions.outcome?.ok ?? true} text={actions.outcome?.text ?? null} testId="notifications-outcome" />
      {shown.pending.length === 0 && shown.fresh.length === 0 && shown.earlier.length === 0 && (
        <EmptyState testId="notifications-empty" title="No notifications yet" message="Contact requests and payments from your contacts show here." />
      )}
      {shown.pending.length > 0 && <Requests contacts={shown.pending} more={0} actions={actions} hasKeys={hasKeys} onOpen={openContact} onDone={onDone} testId="notifications-requests" />}
      {shown.fresh.length > 0 && <Events title="New" events={shown.fresh} fresh more={0} nameOf={nameOf} onOpen={openEvent} testId="notifications-new" />}
      {shown.earlier.length > 0 && <Events title="Earlier" events={shown.earlier} fresh={false} more={0} nameOf={nameOf} onOpen={openEvent} testId="notifications-earlier" />}
      {feed.hasNextPage && (
        <DashButton variant="plainBlue" size="small" data-testid="notifications-more" disabled={feed.isFetchingNextPage} onClick={() => void feed.fetchNextPage()}>
          {feed.isFetchingNextPage ? "Loading…" : "Show more"}
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
  onDone: (outcome: ActionOutcome | null) => void;
  testId: string;
}) {
  const titleId = useId();
  return (
    <section className="bell-section" aria-labelledby={titleId} data-testid={testId}>
      <h3 className="bell-section-title" id={titleId}>
        Contact requests
      </h3>
      <ul className="bell-rows">
        {contacts.map((c) => {
          const sub = subtitle(c);
          return (
            <li key={c.contact} className="bell-row has-actions" data-testid="bell-request" data-contact={c.contact}>
              <button type="button" className="bell-row-open" onClick={() => onOpen(c.contact)}>
                <Avatar letters={avatarLetters(c)} />
                <span className="bell-row-text">
                  <span className="bell-row-title">{shownName(c)}</span>
                  {sub && <span className="bell-row-sub">{sub}</span>}
                </span>
              </button>
              <RequestButtons contact={c} actions={actions} hasKeys={hasKeys} testPrefix="bell" onDone={onDone} />
            </li>
          );
        })}
      </ul>
      {more > 0 && <p className="bell-more">{more === 1 ? "1 more request" : `${more} more requests`}</p>}
      {hasKeys === false && (
        <p className="bell-more" data-testid={`${testId}-enable-first`}>
          {ENABLE_FIRST} Contacts offers it.
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
  const titleId = useId();
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
              <span className="bell-row-text">
                <span className="bell-row-title">{eventText(e, nameOf)}</span>
                <span className="bell-row-sub">{detailDate(new Date(e.at * 1000))}</span>
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
      {more > 0 && <p className="bell-more">{more === 1 ? "1 more" : `${more} more`}</p>}
    </section>
  );
}
