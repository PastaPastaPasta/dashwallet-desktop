// Pay ▸ To a contact (DP3-06), the logic without React: whose contacts the tab pays and why it
// cannot pay one now (the banner, D6), the picker's rows (D4), the payee's names, "Accept and
// pay"'s steps under one credential and one lease (D10, D11) and what Check again found (D7). The
// confirm, check and result logic is U-03's (model.ts) with its contact branch.

import { hasCode, type Backend } from "../../api/backend";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactList } from "../../api/generated/ContactList";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { CredentialSpec } from "../../api/generated/CredentialSpec";
import type { LockResolutionName } from "../../api/generated/LockResolutionName";
import type { LockStateName } from "../../api/generated/LockStateName";
import { actionError } from "../../contacts/actions";
import { avatarLetters, shownName, subtitle, type ContactsOwner } from "../../contacts/model";
import { ENABLE_FIRST, payNote } from "../../contacts/pane";

/** The frequent strip's length (D3; the engine clamps to 1–20). */
export const FREQUENT_LIMIT = 8;

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

export function noOwner(owner: Exclude<ContactsOwner, { kind: "identity" }>): NoOwner | null {
  switch (owner.kind) {
    case "starting":
      return null;
    case "error":
      return { title: "Could not read DashPay's state", message: null, next: "retry" };
    case "registering":
      return { title: "Your username is being registered", message: "You can pay your contacts once Dash Platform confirms it.", next: null };
    case "join":
      if (owner.reason === "watchOnly") return { title: "No DashPay contacts", message: "A watch-only wallet cannot pay contacts.", next: null };
      if (owner.reason === "waitingForSync") return { title: "Join DashPay", message: "You can join DashPay once the wallet has synced.", next: null };
      return { title: "Join DashPay", message: "Create a username to add contacts and pay them by name.", next: "join" };
  }
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

const GROUP_TITLES = { contacts: "My Contacts", requests: "Contact Requests" } as const;

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

function rowReason(c: Pick<ContactSummaryInfo, "relation" | "channel">): string | null {
  if (c.relation === "pendingIncoming") return "Accept and pay";
  return c.channel === "broken" ? "Payments cannot be sent right now" : null;
}

// ---------------------------------------------------------------- the banner (D6)

/** Why the payee cannot be paid now, and what to do; `note` does not hold the payment back. */
export type Banner =
  | { kind: "loading" }
  | { kind: "readFailed"; text: string }
  | { kind: "gone"; text: string }
  | { kind: "watchOnly"; text: string }
  | { kind: "enable"; text: string }
  | { kind: "paymentLock"; text: string; txid: string }
  | { kind: "broken"; text: string }
  | { kind: "mixingOnly"; text: string }
  | { kind: "note"; text: string };

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

export const LOCK_TEXT = "A payment to this contact may have been sent. Check it again before you pay again.";

/** First match wins (D6). A locked vault never reaches the tab: the lock screen covers it. */
export function payBanner(i: BannerInput): Banner | null {
  if (i.detail === undefined) return i.readError ? { kind: "readFailed", text: `Could not read this contact: ${i.readError}` } : { kind: "loading" };
  if (i.detail === null) return { kind: "gone", text: "This user is no longer in your contacts." };
  const { relation, channel } = i.detail.summary;
  const request = relation === "pendingIncoming" && !i.accepted;
  if (relation !== "contact" && !request && !i.accepted) return { kind: "gone", text: "You can pay this user once you are contacts." };
  if (i.watchOnly) return { kind: "watchOnly", text: "A watch-only wallet cannot pay contacts." };
  if (request && i.hasKeys === false) return { kind: "enable", text: `${ENABLE_FIRST} Then you can accept and pay.` };
  if (i.detail.paymentLock) return { kind: "paymentLock", text: LOCK_TEXT, txid: i.detail.paymentLock.txid };
  if (channel === "broken") return { kind: "broken", text: "Payments to this contact cannot be sent right now." };
  if (i.lock === "unlockedMixingOnly") return { kind: "mixingOnly", text: "The wallet is unlocked for mixing only. Unlock it fully to pay." };
  if (request) return { kind: "note", text: "This user sent you a contact request. Accept and pay accepts it and pays in one step." };
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
      return { text: "The payment was sent. You can pay this contact again.", resolved: true };
    case "notSent":
      return { text: "The payment was not sent. You can pay this contact again.", resolved: true };
    case "unknown":
      return { text: "Still not known. Check again once the wallet has synced.", resolved: false };
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

/** One credential check for both grants, one `acceptAndPay` lease, then the accept under it:
 * returns the lease the payment's `prepare` takes. `contact.already_contact` (another surface or
 * device accepted first) is what was asked for. On a refusal nothing is left: the grants are
 * revoked, or the lease ended. */
export async function acceptUnderLease(
  api: AcceptApi,
  a: { walletId: string; identity: string; from: string; spendCap: number; credential: CredentialSpec },
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
    );
  } catch (e) {
    for (const g of grants) void api.revokeGrant(g.id).catch(() => null);
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
export const acceptedText = (name: string) => `You and ${name} are now contacts.`;

/** An accept's refusal: DP2-09's wording; nothing was paid. */
export const acceptFailedText = (e: unknown) => `${actionError(e)} Nothing was paid.`;
