// Pay ▸ To a contact (DP3-06), the logic without React: whose contacts the tab pays and why it
// cannot pay one now (the banner, D6), the picker's rows (D4), the payee's names, "Accept and
// pay"'s steps under one credential and one lease (D10, D11) and what Check again found (D7). The
// confirm, check and result logic is U-03's (model.ts) with its contact branch.

import { hasCode, type Backend } from "../../api/backend";
import type { AppError } from "../../api/generated/AppError";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactList } from "../../api/generated/ContactList";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { CredentialSpec } from "../../api/generated/CredentialSpec";
import type { EligibilityName } from "../../api/generated/EligibilityName";
import type { LockResolutionName } from "../../api/generated/LockResolutionName";
import type { LockStateName } from "../../api/generated/LockStateName";
import type { PaymentSourceName } from "../../api/generated/PaymentSourceName";
import type { UserHit } from "../../api/generated/UserHit";
import type { UserRelation } from "../../api/generated/UserRelation";
import type { PayTab } from "../../app/ui";
import { actionError } from "../../contacts/actions";
import { cardLine } from "../../contacts/add";
import { avatarLetters, networkHits, shownName, subtitle, type ContactsOwner } from "../../contacts/model";
import { payNote } from "../../contacts/pane";
import { messageTable, t } from "../../i18n";
import { en } from "../../i18n/en";
import type { PayNotice } from "./model";

type Named = Pick<ContactSummaryInfo, "contact" | "alias" | "displayName" | "username">;

/** How a payee shows on the strip, in the picker, on the payee card and the confirm sheet (D5). */
export interface Payee {
  contact: string;
  name: string;
  /** `@username`, when the name is not it. */
  sub: string | null;
  letters: string;
}

export function payeeOf(c: Named): Payee {
  return { contact: c.contact, name: shownName(c), sub: subtitle(c), letters: avatarLetters(c) };
}

/** The tab without a main identity (D1): what it says and offers instead of the picker. */
export interface NoOwner {
  title: string;
  message: string | null;
  next: "join" | "retry" | null;
}

export function noOwner(owner: Exclude<ContactsOwner, { kind: "identity" | "starting" }>): NoOwner {
  switch (owner.kind) {
    case "error":
      return { title: t("pay.contact.noOwner.error"), message: null, next: "retry" };
    case "registering":
      return { title: t("pay.contact.noOwner.registering"), message: t("pay.contact.noOwner.registeringText"), next: null };
    case "join":
      if (owner.reason === "watchOnly") return { title: t("pay.contact.noOwner.watchOnly"), message: t("pay.contact.watchOnly"), next: null };
      if (owner.reason === "waitingForSync") return { title: t("pay.contact.noOwner.join"), message: t("pay.contact.noOwner.waitingForSync"), next: null };
      return { title: t("pay.contact.noOwner.join"), message: t("pay.contact.noOwner.createUsername"), next: "join" };
  }
}

/** The tab Pay opens on when nothing named one: To a contact only when there is someone to pay
 * there (a contact or a request), else To an address; `null` while that is still being read
 * (`owner` is `undefined` until DashPay's status is read). */
export function defaultPayTab(
  owner: (ContactsOwner & { failed?: boolean; fetching?: boolean }) | undefined,
  list: { data: ContactList | undefined; failed: boolean; fetching?: boolean },
): PayTab | null {
  // The tab is chosen once, so not from a cached answer that a running read may yet overturn.
  if (owner === undefined || owner.fetching || list.fetching) return null;
  // A failed read keeps the last good answer, which may be stale: To an address, not that list.
  if (owner.kind !== "identity" || owner.failed || list.failed) return "address";
  if (list.data === undefined) return null;
  return pickerGroups(list.data).length > 0 ? "contact" : "address";
}

// ---------------------------------------------------------------- the picker (D4)

export interface PickerRow extends Payee {
  /** Why it cannot be paid now (selecting it shows the banner), else `null`. */
  reason: string | null;
}

export interface PickerGroup {
  id: "contacts" | "requests";
  title: string;
  rows: PickerRow[];
}

const GROUP_TITLES = messageTable("pay.contact.groups", ["contacts", "requests"]);

/** The picker's groups from the engine's `contacts` and `requests` sections, in its order; the
 * other sections are not payable here. An empty group is left out. */
export function pickerGroups(list: ContactList | undefined): PickerGroup[] {
  const groups: PickerGroup[] = [];
  for (const id of ["contacts", "requests"] as const) {
    const rows = (list?.sections.find((s) => s.section === id)?.contacts ?? []).map((c) => ({ ...payeeOf(c), reason: rowReason(c) }));
    if (rows.length > 0) groups.push({ id, title: GROUP_TITLES[id], rows });
  }
  return groups;
}

/** What the picker shows under the search field: the contact groups, the username search's
 * section (while it runs, failed, or found someone), and which "nothing matches" it says:
 * `contacts` before the username search can run (under two characters) while there are contacts,
 * `short` then without any, `anyone` once neither found anyone. */
export function pickerView(i: {
  text: string;
  groups: number;
  /** Whether the identity has any contact or request, whatever the text. */
  anyContacts: boolean;
  /** Whether the username search runs for the text (two characters or more). */
  network: boolean;
  networkState: "loading" | "error" | "done";
  /** The hits the section lists. */
  hits: number;
}): { network: boolean; none: "contacts" | "short" | "anyone" | null } {
  const network = i.network && (i.networkState !== "done" || i.hits > 0);
  if (i.text === "" || i.groups > 0) return { network, none: null };
  if (!i.network) return { network, none: i.anyContacts ? "contacts" : "short" };
  // No group and no hit lists nothing, even when the search matched contacts the picker's own
  // filter did not (it folds homographs, the filter does not).
  return { network, none: i.networkState === "done" && i.hits === 0 ? "anyone" : null };
}

/** The username search's hits under the picker: everyone the picker itself does not list for the
 * text, whatever their relation. Matched against the filtered list the picker shows, not the
 * identity's whole list: the search folds homographs, the filter does not, so a contact the
 * filter missed is found here. */
export function pickerHits(hits: UserHit[] | undefined, shown: ContactList): { shown: UserHit[]; more: boolean; count: number } {
  return networkHits(hits, { ...shown, sections: shown.sections.filter((s) => s.section === "contacts" || s.section === "requests") }, true);
}

/** The list the username search's hits are matched against: the picker's list for the text once
 * read (not the previous text's, kept while it loads); an empty one when that read failed with
 * nothing kept, as the picker then lists no one; `null` while it is still being read. */
export function pickerListing(list: { data: ContactList | undefined; placeholder: boolean; failed: boolean }): ContactList | null {
  if (list.data !== undefined) return list.placeholder ? null : list.data;
  return list.failed ? { sections: [], updating: false } : null;
}

/** Where the username search under the picker stands. It is still running until the search has
 * answered for the text shown and the picker's list for that text is read (`listReady`): before,
 * its hits cannot be told apart from the picker's rows. */
export function pickerNetworkState(i: { listReady: boolean; caughtUp: boolean; pending: boolean; failed: boolean; fetching: boolean }): "loading" | "error" | "done" {
  // A re-read (after a contact request, say) may change a hit's relation: not the old one meanwhile.
  if (!i.listReady || !i.caughtUp || i.pending || i.fetching) return "loading";
  return i.failed ? "error" : "done";
}

/** What the picker's live region says once the username search answered; `null` before. */
export function searchAnnouncement(i: { network: boolean; networkState: "loading" | "error" | "done"; hits: number; none: ReturnType<typeof pickerView>["none"] }): string | null {
  if (i.none === "anyone") return t("pay.contact.noOneFound");
  if (!i.network || i.networkState === "loading") return null;
  if (i.networkState === "error") return t("pay.contact.searchFailed");
  if (i.hits === 0) return null;
  return t("pay.contact.peopleFound", { count: i.hits });
}

function rowReason(c: Pick<ContactSummaryInfo, "relation" | "channel">): string | null {
  if (c.relation === "pendingIncoming") return t("pay.contact.rowAcceptAndPay");
  return c.channel === "broken" ? t("pay.contact.rowBroken") : null;
}

// ---------------------------------------------------------------- a user found by username

/** What the card of a user the search found offers: send them a contact request, pay them
 * (already a contact), accept and pay their request, enable DashPay's keys, or check again. */
export type FoundNext = "send" | "pay" | "acceptAndPay" | "enable" | "retry" | null;

export interface FoundLine {
  text: string | null;
  next: FoundNext;
}

/** Add contact's card line (`cardLine`) in Pay's words; `undefined` while the check runs. A
 * DashPay payment needs the payee's request, so a request sent is paid once they accept. */
export function foundLine(name: string, relation: UserRelation, eligibility: { data: EligibilityName | undefined; failed: boolean }): FoundLine | undefined {
  const line = cardLine(relation, eligibility);
  if (!line) return undefined;
  switch (line.next) {
    case "send":
      return { text: t("pay.contact.foundSend", { name }), next: "send" };
    case "showContact":
      return { text: line.text, next: "pay" };
    case "showRequest":
      return eligibility.data === "pendingIncoming"
        ? { text: line.text, next: "acceptAndPay" }
        : { text: t("pay.contact.foundRequested", { name }), next: null };
    case "enable":
    case "retry":
      return { text: line.text, next: line.next };
    default:
      return { text: line.text, next: null };
  }
}

// ---------------------------------------------------------------- the banner (D6)

/** Why the payee cannot be paid now, and what to do; `note` does not hold the payment back. A
 * `PayNotice` comes from a refused payment, never from `payBanner`. */
export type Banner =
  | { kind: "loading" }
  | { kind: "readFailed"; text: string }
  | { kind: "gone"; text: string }
  | { kind: "watchOnly"; text: string }
  | { kind: "enable"; text: string }
  | { kind: "paymentLock"; text: string; txid: string; source: PaymentSourceName }
  | { kind: "broken"; text: string }
  | { kind: "mixingOnly"; text: string }
  | { kind: "note"; text: string }
  | PayNotice;

export interface BannerInput {
  /** The payee's detail: `undefined` while unread, `null` when the engine has no such contact. */
  detail: ContactDetailInfo | null | undefined;
  /** The detail read failed (and holds nothing). */
  readError: string | null;
  /** `undefined` while the wallet list is unread. */
  watchOnly: boolean | undefined;
  lock: LockStateName | undefined;
  /** Whether the main identity has its DashPay keys (`undefined`: unknown). */
  hasKeys: boolean | undefined;
  /** This form accepted the payee's request already (D11). */
  accepted: boolean;
}

export const PAYMENT_LOCK_TEXT = en.pay.contact.banner.paymentLock;

/** The locked banner names the balance a payout left from (C-funding D-C5 Copy). */
function lockText(source: PaymentSourceName): string {
  switch (source) {
    case "platform":
      return t("pay.contact.banner.paymentLockPlatform");
    case "shielded":
      return t("pay.contact.banner.paymentLockShielded");
    case "core":
      return t("pay.contact.banner.paymentLock");
  }
}

/** First match wins (D6). A locked vault never reaches the tab: the lock screen replaces the pages. */
export function payBanner(i: BannerInput): Banner | null {
  if (i.detail === undefined) return i.readError ? { kind: "readFailed", text: t("pay.contact.banner.readFailed", { error: i.readError }) } : { kind: "loading" };
  if (i.detail === null) return { kind: "gone", text: t("pay.contact.banner.gone") };
  const { relation, channel } = i.detail.summary;
  const request = acceptsToo(i.detail, i.accepted);
  if (relation !== "contact" && !request && !i.accepted) return { kind: "gone", text: t("pay.contact.banner.notContacts") };
  if (i.watchOnly) return { kind: "watchOnly", text: t("pay.contact.watchOnly") };
  if (request && i.hasKeys === false) return { kind: "enable", text: t("pay.contact.banner.enableThenAccept") };
  if (i.detail.paymentLock) return { kind: "paymentLock", text: lockText(i.detail.paymentLock.source), txid: i.detail.paymentLock.txid, source: i.detail.paymentLock.source };
  if (channel === "broken") return { kind: "broken", text: t("contacts.pane.broken") };
  if (i.lock === "unlockedMixingOnly") return { kind: "mixingOnly", text: t("pay.contact.banner.mixingOnly") };
  if (request) return { kind: "note", text: t("pay.contact.banner.request") };
  const note = payNote(channel);
  return note ? { kind: "note", text: note } : null;
}

export const blocks = (b: Banner | null) => b !== null && b.kind !== "note";

/** Whether Review accepts the payee's request too (D10, D11). */
export function acceptsToo(detail: ContactDetailInfo | null | undefined, accepted: boolean): boolean {
  return detail?.summary.relation === "pendingIncoming" && !accepted;
}

/** What Check again found (D7). */
export function resolveText(r: LockResolutionName): { text: string; resolved: boolean } {
  switch (r) {
    case "sent":
      return { text: t("pay.contact.resolve.sent"), resolved: true };
    case "notSent":
      return { text: t("pay.contact.resolve.notSent"), resolved: true };
    case "unknown":
      return { text: t("pay.contact.resolve.unknown"), resolved: false };
  }
}

// ---------------------------------------------------------------- Accept and pay (D10–D12)

export type AcceptApi = Pick<Backend, "grantRequest" | "authorizeSet" | "beginFlow" | "dashpayAcceptRequest" | "endFlow" | "revokeGrant">;

/** Where Accept and pay stopped: `authorize` is the credential's refusal (the sheet asks again),
 * `accept` the request's (nothing was accepted or paid). */
export class AcceptFailed extends Error {
  constructor(
    readonly stage: "authorize" | "accept",
    readonly cause: unknown,
  ) {
    super(stage);
  }
}

/** The signing window ended before the lease took its grants: asked again with the passphrase. */
const WINDOW_ENDED: AppError = { code: "vault.credential_required", message: "The signing window ended. Enter the passphrase.", data: null };

/** One credential check for both grants, one `acceptAndPay` lease, then the accept under it:
 * returns the lease the payment's `prepare` takes. `contact.already_contact` (another surface or
 * device accepted first) is what was asked for. On a refusal nothing is left: the grants are
 * revoked, or the lease ended. */
export async function acceptUnderLease(
  api: AcceptApi,
  a: {
    walletId: string;
    identity: string;
    from: string;
    spendCap: number;
    credential: CredentialSpec;
    /** The grants come from an encrypted vault's signing window (no passphrase typed): a window
     * that ends before the lease drops them, and the passphrase is what asks again. */
    windowed?: boolean;
  },
): Promise<string> {
  let grants: AuthGrant[];
  try {
    const { maxDuffs, maxCredits } = await api.grantRequest(a.walletId, a.identity, { kind: "acceptRequest" });
    grants = await api.authorizeSet(
      [
        { kind: "platformOp", maxDuffs, maxCredits },
        { kind: "spend", maxDuffs: a.spendCap },
      ],
      a.walletId,
      a.credential,
    );
  } catch (e) {
    throw new AcceptFailed("authorize", e);
  }
  let lease: string;
  try {
    lease = await api.beginFlow(
      a.walletId,
      "acceptAndPay",
      grants.map((g) => g.id),
      null,
    );
  } catch (e) {
    for (const g of grants) void api.revokeGrant(g.id).catch(() => null);
    // The signing window ended between the grants and the lease (issue 173, item 9): its grants went with
    // it. Nothing was accepted or paid, so the passphrase is asked for, as when the window is closed.
    // An unencrypted vault has no window and no passphrase to ask for: its refusal stays the accept's.
    if (a.windowed && a.credential.kind === "unencrypted" && (hasCode(e, "platform.grant_invalid") || hasCode(e, "vault.grant_invalid")))
      throw new AcceptFailed("authorize", WINDOW_ENDED);
    throw new AcceptFailed("accept", e);
  }
  try {
    await api.dashpayAcceptRequest(a.walletId, a.identity, a.from, lease);
  } catch (e) {
    if (!hasCode(e, "contact.already_contact")) {
      void api.endFlow(lease).catch(() => null);
      throw new AcceptFailed("accept", e);
    }
  }
  return lease;
}

/** The result's words for an accept that went through (D12, E0-04 C8). */
export const acceptedText = (name: string) => t("pay.contact.accepted", { name });

/** An accept's refusal: DP2-09's wording; nothing was paid. */
export const acceptFailedText = (e: unknown) => `${actionError(e)} ${t("pay.contact.nothingPaid")}`;
