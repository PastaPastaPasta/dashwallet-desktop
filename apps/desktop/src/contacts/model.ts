// The Contacts screen's view model (DASHPAY §4 `ContactsViewModel`, DP2-07): which identity's
// contacts show, how the engine's sections become the screen's groups, the names and chips of a
// row, the network search under them, the pending-setup hint and keyboard moves. Plain
// TypeScript over the generated types; the queries are in queries.ts and the screen in
// screens/contacts. Copy follows the mobile wallets (iOS ContactsScreen, Android strings-dashpay).

import type { ChannelStateName } from "../api/generated/ChannelStateName";
import type { ContactList } from "../api/generated/ContactList";
import type { ContactSectionName } from "../api/generated/ContactSectionName";
import type { ContactSortName } from "../api/generated/ContactSortName";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { LockStateName } from "../api/generated/LockStateName";
import type { UserHit } from "../api/generated/UserHit";
import type { UserRelation } from "../api/generated/UserRelation";
import type { Tone } from "../components/basics";

/** Whose contacts the page shows (D2), or why it shows none. */
export type ContactsOwner =
  | { kind: "identity"; identity: string }
  /** No identity: Join DashPay (`reason` as the status gives it). */
  | { kind: "join"; reason: "watchOnly" | "waitingForSync" | null }
  | { kind: "registering" }
  /** DashPay has not said yet, or started without a main identity it could name. */
  | { kind: "starting" }
  /** The status (or the identities it needs) could not be read. */
  | { kind: "error" };

export function contactsOwner(status: DashPayStatus | undefined): ContactsOwner {
  switch (status?.kind) {
    case "ready":
      return { kind: "identity", identity: status.main };
    // A contested name still has its identity (U-01's sidebar rule).
    case "contestPending":
      return { kind: "identity", identity: status.identity };
    case "noIdentity":
      return { kind: "join", reason: status.reason };
    case "registering":
      return { kind: "registering" };
    // The engine reports it only while no identity is on file (BL-153 D2).
    case "startupIncomplete":
    default:
      return { kind: "starting" };
  }
}

/** Android's four orders (UsernameSortOrderBy), as the "Sort by" menu names them (AND-090). */
export const SORTS: { id: ContactSortName; title: string }[] = [
  { id: "displayName", title: "Name" },
  { id: "username", title: "Username" },
  { id: "dateAdded", title: "Date added" },
  { id: "lastActivity", title: "Last activity" },
];

const nameOf = (c: Pick<ContactSummaryInfo, "alias" | "displayName" | "username">) => c.alias?.trim() || c.displayName?.trim() || c.username?.trim();

/** The name a row shows: the alias, the display name, the username, else the shortened id (the
 * engine's `DisplayName` key, so the list reads in the order it sorts). */
export function shownName(c: Pick<ContactSummaryInfo, "alias" | "displayName" | "username" | "contact">): string {
  return nameOf(c) || shortId(c.contact);
}

/** `@username` under the name, when the name is not the username itself. */
export function subtitle(c: Pick<ContactSummaryInfo, "alias" | "displayName" | "username" | "contact">): string | null {
  const username = c.username?.trim();
  return username && username !== shownName(c) ? `@${username}` : null;
}

/** An identity id cut to its ends, for a row with no name yet. */
export function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 6)}…${id.slice(-4)}` : id;
}

/** The avatar's letters (D10): the initials of the name shown, `?` for a row known only by its id. */
export function avatarLetters(c: Pick<ContactSummaryInfo, "alias" | "displayName" | "username">): string {
  return initials(nameOf(c) ?? "");
}

/** Up to two letters of a name; `?` when it has none. */
export function initials(name: string): string {
  // The first letter of each word (a letter, not half of a surrogate pair), first and last word.
  const firsts = (name.match(/\p{L}[\p{L}\p{M}'’-]*/gu) ?? []).map((w) => String.fromCodePoint(w.codePointAt(0)!).toUpperCase());
  const out = firsts.length > 1 ? firsts[0] + firsts[firsts.length - 1] : (firsts[0] ?? "");
  return out || "?";
}

export interface RowChip {
  id: "settingUp" | "unavailable" | "unverified";
  text: string;
  tone: Tone;
  /** What the chip means, for its tooltip. */
  detail: string;
}

/** A row's chips (D7): the payment channel when it is not ready, and trust. */
export function rowChips(c: Pick<ContactSummaryInfo, "relation" | "channel" | "unverified">): RowChip[] {
  const chips: RowChip[] = [];
  if (c.relation === "contact" && c.channel === "setupPending")
    chips.push({ id: "settingUp", text: "Setting up", tone: "info", detail: "Payments with this contact are still being set up." });
  if (c.relation === "contact" && c.channel === "broken")
    chips.push({ id: "unavailable", text: "Payments unavailable", tone: "danger", detail: "Payments to this contact cannot be sent right now." });
  if (c.unverified)
    chips.push({ id: "unverified", text: "Unverified", tone: "warning", detail: "Not yet verified against the Dash network." });
  return chips;
}

const RELATIONS: Record<UserRelation, string> = {
  contact: "Contact",
  pendingOutgoing: "Request sent",
  pendingIncoming: "Request received",
  ignored: "Ignored",
  stranger: "Not a contact",
  isSelf: "You",
};

/** A row's trailing note (D7): which way a request goes, or an ignored sender. */
export function rowNote(c: Pick<ContactSummaryInfo, "relation">): string | null {
  return c.relation === "pendingIncoming" || c.relation === "pendingOutgoing" || c.relation === "ignored" ? RELATIONS[c.relation] : null;
}

/** One group of the list, as the screen draws it. */
export interface Group {
  section: ContactSectionName;
  title: string;
  contacts: ContactSummaryInfo[];
  /** Only Hidden collapses (D5), and not while a search shows it. */
  collapsible: boolean;
  open: boolean;
}

const TITLES: Record<ContactSectionName, (n: number) => string> = {
  requests: (n) => `Contact Requests (${n})`,
  contacts: () => "My Contacts",
  pending: () => "Pending Requests",
  hidden: (n) => `Hidden (${n})`,
};

/** The engine's sections as groups, in its order, empty ones dropped (D1). Hidden is open when
 * the user opened it; a search shows its matches, open and not collapsible (D5). */
export function groups(list: ContactList | undefined, opts: { hiddenOpen: boolean; searching: boolean }): Group[] {
  return (list?.sections ?? [])
    .filter((s) => s.contacts.length > 0)
    .map((s) => {
      const collapsible = s.section === "hidden" && !opts.searching;
      return {
        section: s.section,
        title: TITLES[s.section](s.contacts.length),
        contacts: s.contacts,
        collapsible,
        open: !collapsible || opts.hiddenOpen,
      };
    });
}

/** The row of contact `id` in any group; `null` when the list does not hold it. */
export function findContact(list: ContactList | undefined, id: string | null): ContactSummaryInfo | null {
  if (id === null) return null;
  return list?.sections.flatMap((s) => s.contacts).find((c) => c.contact === id) ?? null;
}

/** Local search debounce (Activity's), network search debounce and limit (iOS), and how many
 * network rows show before "Show more results". */
export const SEARCH_DEBOUNCE_MS = 250;
export const NETWORK_DEBOUNCE_MS = 350;
export const NETWORK_LIMIT = 12;
const NETWORK_SHOWN = 3;
const NETWORK_MIN_CHARS = 2;

/** The prefix the network search asks for (D6); `null` below two characters. */
export function networkPrefix(text: string): string | null {
  const t = text.trim();
  return [...t].length >= NETWORK_MIN_CHARS ? t : null;
}

/** The network rows under the local groups: strangers only (the identity itself and everyone it
 * relates to are in the groups already, whatever the local filter matched), at most three; `more`
 * when others remain (iOS `networkResultsSection`). `listed` is everyone the current contact list
 * holds: a cached hit read before its request or contact arrived still says `stranger` (D21). */
export function networkHits(hits: UserHit[] | undefined, listed: ContactList | undefined): { shown: UserHit[]; more: boolean } {
  const known = new Set(listed?.sections.flatMap((s) => s.contacts.map((c) => c.contact)));
  const fresh = (hits ?? []).filter((h) => h.relation === "stranger" && !known.has(h.identity));
  return { shown: fresh.slice(0, NETWORK_SHOWN), more: fresh.length > NETWORK_SHOWN };
}

/** A relation as the detail slot names it. */
export function relationTitle(r: UserRelation): string {
  return RELATIONS[r];
}

const CHANNELS: Record<ChannelStateName, string> = {
  notEstablished: "Not set up",
  setupPending: "Setting up",
  ready: "Ready",
  broken: "Unavailable",
};

/** The DIP-15 payment channel as the detail slot names it. */
export function channelTitle(c: ChannelStateName): string {
  return CHANNELS[c];
}

/** The pending-setup hint (D8). */
export interface SetupHint {
  kind: "unlock" | "finishing";
  text: string;
}

const contactsWord = (n: number) => (n === 1 ? "1 contact" : `${n} contacts`);

export function setupHint(count: number | undefined, lock: LockStateName | undefined): SetupHint | null {
  if (!count || count <= 0 || lock === undefined) return null;
  if (lock === "unlockedMixingOnly") return { kind: "unlock", text: `Unlock to finish setting up ${contactsWord(count)}` };
  if (lock === "unlocked" || lock === "unencrypted") return { kind: "finishing", text: `Finishing the setup of ${contactsWord(count)}…` };
  return null;
}

/** Where a key moves the focus among `ids` (the rows in screen order), from `current`; `null`
 * when the key does not move it (D13). */
export function moveFocus(ids: string[], current: string | null, key: string): string | null {
  if (ids.length === 0) return null;
  const i = current === null ? -1 : ids.indexOf(current);
  switch (key) {
    case "ArrowDown":
      return ids[Math.min(i + 1, ids.length - 1)];
    case "ArrowUp":
      return ids[i <= 0 ? 0 : i - 1];
    case "Home":
      return ids[0];
    case "End":
      return ids[ids.length - 1];
    default:
      return null;
  }
}
