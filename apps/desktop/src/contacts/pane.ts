// The contact pane's view model (DASHPAY §4 `ContactDetailViewModel`, DP2-09): which actions a
// contact's relation offers and why one is held back, the Enable DashPay banner, the payment
// activity's rows and what the detail slot announces. Plain TypeScript over the generated types;
// the actions run in actions.ts, the screen is screens/contacts/ContactPane.tsx.

import type { ActivityFilterName } from "../api/generated/ActivityFilterName";
import type { ContactActivityItem } from "../api/generated/ContactActivityItem";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { CostTable } from "../api/generated/CostTable";
import type { PrivateContactDetails } from "../api/generated/PrivateContactDetails";
import { relationTitle, shownName } from "./model";

/** What the pane offers (D2): a request to answer, one sent, a contact, or an ignored sender. */
export type PaneActions =
  | { kind: "request"; acceptBlocked: string | null }
  | { kind: "sent" }
  /** `payNote`: why a payment may not go through yet, Pay still offered (DP3-06 D15). */
  | { kind: "contact"; payBlocked: string | null; payNote: string | null }
  | { kind: "ignored" }
  | { kind: "none" };

export const ENABLE_FIRST = "Enable DashPay first.";

/** The actions of `c`'s relation; `hasKeys` is whether the main identity has its DashPay keys
 * (`undefined` while unknown: Accept stays offered and the engine refuses if need be). */
export function paneActions(c: Pick<ContactSummaryInfo, "relation" | "channel">, hasKeys: boolean | undefined): PaneActions {
  switch (c.relation) {
    case "pendingIncoming":
      return { kind: "request", acceptBlocked: hasKeys === false ? ENABLE_FIRST : null };
    case "pendingOutgoing":
      return { kind: "sent" };
    case "contact":
      return { kind: "contact", payBlocked: c.channel === "broken" ? BROKEN : null, payNote: payNote(c.channel) };
    case "ignored":
      return { kind: "ignored" };
    default:
      return { kind: "none" };
  }
}

const BROKEN = "Payments to this contact cannot be sent right now.";

/** D9, by the DIP-15 channel: only a broken one holds Pay back (DP3-01 D19: a payment finishes a
 * channel's setup); the others say why a payment may not go through yet. */
export function payNote(channel: ContactSummaryInfo["channel"]): string | null {
  switch (channel) {
    case "ready":
    case "broken":
      return null;
    case "setupPending":
      return "Payments with this contact are still being set up. The payment finishes the setup.";
    case "notEstablished":
      return "Payments with this contact are not set up yet, so a payment may not go through.";
  }
}

/** The private details' form (D7), from what the engine holds. */
export interface DetailsForm {
  alias: string;
  note: string;
  hidden: boolean;
}

export function detailsForm(d: PrivateContactDetails | undefined): DetailsForm {
  return { alias: d?.alias ?? "", note: d?.note ?? "", hidden: d?.hidden ?? false };
}

/** The details a form saves: trimmed, blank is none (the engine trims too). */
export function detailsOf(f: DetailsForm): PrivateContactDetails {
  const clean = (v: string) => v.trim() || null;
  return { alias: clean(f.alias), note: clean(f.note), hidden: f.hidden };
}

/** Whether the form differs from what was saved. */
export function detailsChanged(f: DetailsForm, saved: PrivateContactDetails | undefined): boolean {
  const a = detailsOf(f);
  const b = detailsOf(detailsForm(saved));
  return a.alias !== b.alias || a.note !== b.note || a.hidden !== b.hidden;
}

/** The Enable DashPay banner (D10): the main identity, when it lacks its DashPay keys (not while
 * that is unknown); `fee` the upgrade's cost in duffs, `null` while unknown. */
export interface EnableBanner {
  identity: string;
  fee: number | null;
}

export function enableBanner(identity: string, hasKeys: boolean | undefined, costs: CostTable | undefined): EnableBanner | null {
  if (hasKeys !== false) return null;
  return { identity, fee: costs ? creditsToDuffs(costs.enableDashpayKeys, costs.creditsPerDuff) : null };
}

/** Credits as duffs, rounded up (a fee is never shown smaller than it is). */
export function creditsToDuffs(credits: string, perDuff: number): number {
  const c = BigInt(credits);
  const p = BigInt(Math.max(1, perDuff));
  return Number((c + p - 1n) / p);
}

/** Android's activity filter (AND-093). */
export const ACTIVITY_FILTERS: { id: ActivityFilterName; title: string }[] = [
  { id: "all", title: "All" },
  { id: "sent", title: "Sent" },
  { id: "received", title: "Received" },
];

/** An activity row's title and signed amount. */
export function activityRow(item: ContactActivityItem): { title: string; signed: number; pending: boolean } {
  const sent = item.direction === "sent";
  return { title: sent ? "Sent" : "Received", signed: sent ? -item.amount : item.amount, pending: item.height === null };
}

/** D14: what the detail slot announces when the selection changes. */
export function selectionAnnouncement(c: Pick<ContactSummaryInfo, "alias" | "displayName" | "username" | "contact" | "relation"> | null): string {
  return c ? `${shownName(c)}, ${relationTitle(c.relation)}` : "";
}
