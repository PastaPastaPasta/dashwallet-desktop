// The bell's view model (DASHPAY §4 `NotificationsViewModel`, DP2-09 D11–D13): an event's line,
// where a click on it leads, the popover's cut of the feed, and what the opening marks read.
// Plain TypeScript over the generated types; the queries are in queries.ts, the screens in
// screens/notifications.

import type { ContactList } from "../api/generated/ContactList";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { DashPayEventPage } from "../api/generated/DashPayEventPage";
import { shortId, shownName } from "../contacts/model";

/** A contact id's name as the identity's contact list shows it, else its shortened id (D13). */
export type NameOf = (contact: string) => string;

export function namesFrom(list: ContactList | undefined): NameOf {
  const byId = new Map<string, ContactSummaryInfo>();
  for (const s of list?.sections ?? []) for (const c of s.contacts) byId.set(c.contact, c);
  return (id) => {
    const c = byId.get(id);
    return c ? shownName(c) : shortId(id);
  };
}

/** D13: an event's line. */
export function eventText(e: Pick<DashPayEventInfo, "kind" | "contact" | "reference">, nameOf: NameOf): string {
  const who = e.contact ? nameOf(e.contact) : "Someone";
  const label = `@${e.reference ?? ""}`;
  switch (e.kind) {
    case "requestReceived":
      return `${who} sent you a contact request`;
    case "requestAccepted":
      return `${who} accepted your contact request`;
    case "contactEstablished":
      return `You and ${who} are now contacts`;
    case "paymentReceived":
      return `${who} sent you a payment`;
    case "usernameRegistered":
      return `Your username ${label} is registered`;
    case "contestWon":
      return `You won the vote for ${label}`;
    case "contestLost":
      return `You lost the vote for ${label}`;
    case "contestLocked":
      return `The vote for ${label} ended locked`;
  }
}

/** Where a click on an event leads (D12): the contact's pane, the payment's transaction, or
 * nowhere (the name events). */
export type EventTarget = { kind: "contact"; contact: string } | { kind: "tx"; txid: string } | null;

export function eventTarget(e: Pick<DashPayEventInfo, "kind" | "contact" | "reference">): EventTarget {
  if (e.kind === "paymentReceived" && e.reference) return { kind: "tx", txid: e.reference };
  if (e.contact && (e.kind === "requestReceived" || e.kind === "requestAccepted" || e.kind === "contactEstablished"))
    return { kind: "contact", contact: e.contact };
  return null;
}

/** The newest event id of a page, for `mark_read` (D11); `null` when it holds none. */
export function newestId(page: Pick<DashPayEventPage, "new" | "earlier"> | undefined): number | null {
  const ids = [...(page?.new ?? []), ...(page?.earlier ?? [])].map((e) => e.id);
  return ids.length ? Math.max(...ids) : null;
}

/** How many requests and new events the popover lists before "N more" (D11). */
export const POPOVER_ROWS = 5;
/** Events one page of the Notifications page asks for (D12). */
export const EVENTS_PAGE = 50;

/** The feed split for a view: an event unread when the view opened stays under New for that
 * opening (`unreadAtOpen`), so marking it read does not move it under the pointer (D11). */
export interface Feed {
  pending: ContactSummaryInfo[];
  fresh: DashPayEventInfo[];
  earlier: DashPayEventInfo[];
}

export function feedOf(pages: DashPayEventPage[], unreadAtOpen: ReadonlySet<number>): Feed {
  const events = pages.flatMap((p) => [...p.new, ...p.earlier]).sort((a, b) => b.id - a.id);
  return {
    pending: pages[0]?.pending ?? [],
    fresh: events.filter((e) => e.readAt === null || unreadAtOpen.has(e.id)),
    earlier: events.filter((e) => e.readAt !== null && !unreadAtOpen.has(e.id)),
  };
}

/** The popover's cut (D11): at most POPOVER_ROWS requests and new events, and how many more. */
export function popoverCut(feed: Feed): { pending: ContactSummaryInfo[]; morePending: number; fresh: DashPayEventInfo[]; moreFresh: number } {
  return {
    pending: feed.pending.slice(0, POPOVER_ROWS),
    morePending: Math.max(0, feed.pending.length - POPOVER_ROWS),
    fresh: feed.fresh.slice(0, POPOVER_ROWS),
    moreFresh: Math.max(0, feed.fresh.length - POPOVER_ROWS),
  };
}
