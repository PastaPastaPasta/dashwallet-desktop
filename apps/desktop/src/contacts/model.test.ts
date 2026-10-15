// The Contacts view model (DP2-07), by parity ID: IOS-072 (the Contacts tab: groups, search,
// hidden, the network search), AND-090 (sort orders, requests section, no-match).

import { describe, expect, it } from "vitest";
import type { ContactList } from "../api/generated/ContactList";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { UserHit } from "../api/generated/UserHit";
import {
  avatarLetters,
  channelTitle,
  contactsOwner,
  findContact,
  groups,
  initials,
  moveFocus,
  networkHits,
  networkPrefix,
  relationTitle,
  rowChips,
  rowNote,
  setupHint,
  shortId,
  shownName,
  SORTS,
  subtitle,
} from "./model";

const ID = (n: number) => `${n}`.padStart(2, "0").repeat(22);

const summary = (n: number, s: Partial<ContactSummaryInfo> = {}): ContactSummaryInfo => ({
  contact: ID(n),
  username: null,
  displayName: null,
  alias: null,
  relation: "contact",
  hidden: false,
  channel: "ready",
  since: null,
  lastActivityAt: null,
  unverified: false,
  ...s,
});

const list = (sections: Partial<Record<"requests" | "contacts" | "pending" | "hidden", ContactSummaryInfo[]>>, updating = false): ContactList => ({
  sections: (["requests", "contacts", "pending", "hidden"] as const).map((section) => ({ section, contacts: sections[section] ?? [] })),
  updating,
});

const hit = (n: number, relation: UserHit["relation"] = "stranger"): UserHit => ({
  identity: ID(n),
  username: `user${n}`,
  displayName: null,
  avatarUrl: null,
  relation,
  unverified: false,
});

describe("Contacts view model", () => {
  it("IOS-072 shows the main identity's contacts, a contested identity's, and the Join prompt without one", () => {
    expect(contactsOwner({ kind: "ready", main: "M" }, undefined)).toEqual({ kind: "identity", identity: "M" });
    expect(contactsOwner({ kind: "contestPending", identity: "C", label: "al", endsAt: null }, undefined)).toEqual({ kind: "identity", identity: "C" });
    expect(contactsOwner({ kind: "noIdentity", reason: "watchOnly" }, undefined)).toEqual({ kind: "join", reason: "watchOnly" });
    expect(contactsOwner({ kind: "registering", draft: "1" }, undefined)).toEqual({ kind: "registering" });
    expect(contactsOwner(undefined, undefined)).toEqual({ kind: "starting" });
    // (A status that cannot be read is the hook's `error`.)
    const main = { identity: "S", isMain: true } as IdentitySummary;
    expect(contactsOwner({ kind: "startupIncomplete", startup: "partialAccountsPending" }, [main])).toEqual({ kind: "identity", identity: "S" });
    expect(contactsOwner({ kind: "startupIncomplete", startup: "starting" }, [])).toEqual({ kind: "starting" });
    // No status from the engine (its stub, D19): the identity list decides.
    expect(contactsOwner(null, [main])).toEqual({ kind: "identity", identity: "S" });
    expect(contactsOwner(null, [])).toEqual({ kind: "join", reason: null });
    expect(contactsOwner(null, undefined)).toEqual({ kind: "starting" });
  });

  it("IOS-072 groups requests, contacts, pending and hidden in that order, with iOS's titles, empty ones dropped", () => {
    const l = list({ requests: [summary(1), summary(2)], contacts: [summary(3)], hidden: [summary(4)] });
    const g = groups(l, { hiddenOpen: false, searching: false });
    expect(g.map((x) => x.title)).toEqual(["Contact Requests (2)", "My Contacts", "Hidden (1)"]);
    expect(g.map((x) => x.open)).toEqual([true, true, false]);
    expect(g.map((x) => x.collapsible)).toEqual([false, false, true]);
    expect(groups(l, { hiddenOpen: true, searching: false }).at(-1)!.open).toBe(true);
    // A search that matches hidden contacts shows them, and Hidden does not collapse meanwhile.
    expect(groups(l, { hiddenOpen: false, searching: true }).at(-1)).toMatchObject({ open: true, collapsible: false });
    expect(groups(undefined, { hiddenOpen: false, searching: false })).toEqual([]);
  });

  it("IOS-072 names a row by alias, display name, username, else the short id, with @username under it", () => {
    expect(shownName(summary(1, { alias: " Judy (work) ", displayName: "Judy", username: "judy" }))).toBe("Judy (work)");
    expect(shownName(summary(1, { alias: " ", displayName: "Bob Builder", username: "bob" }))).toBe("Bob Builder");
    expect(shownName(summary(1, { username: "dave" }))).toBe("dave");
    expect(shownName(summary(7))).toBe(shortId(ID(7)));
    expect(shortId("4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF")).toBe("4EfA9J…iSeF");
    expect(subtitle(summary(1, { displayName: "Bob Builder", username: "bob" }))).toBe("@bob");
    expect(subtitle(summary(1, { username: "dave" }))).toBeNull();
    expect(subtitle(summary(1, { displayName: "X" }))).toBeNull();
  });

  it("IOS-072 avatar initials: first and last word, letters only, ? without any", () => {
    expect(initials("Bob Builder")).toBe("BB");
    expect(initials("carol")).toBe("C");
    expect(initials("Grace Brewster Hopper")).toBe("GH");
    expect(initials("élodie d’arc")).toBe("ÉD");
    expect(avatarLetters(summary(1, { displayName: "Bob Builder", username: "bob" }))).toBe("BB");
    expect(avatarLetters(summary(1, { alias: "Judy (work)", displayName: "Judy" }))).toBe("JW");
    expect(avatarLetters(summary(1))).toBe("?");
    expect(initials("12 34")).toBe("?");
    expect(initials("𝒜lice")).toBe("𝒜");
  });

  it("IOS-072 marks the channel and trust on rows; requests and pending say which way they go", () => {
    expect(rowChips(summary(1, { channel: "setupPending" })).map((c) => c.text)).toEqual(["Setting up"]);
    expect(rowChips(summary(1, { channel: "broken", unverified: true })).map((c) => c.text)).toEqual(["Payments unavailable", "Unverified"]);
    // A request has no channel yet.
    expect(rowChips(summary(1, { relation: "pendingIncoming", channel: "notEstablished" }))).toEqual([]);
    expect(rowNote({ relation: "pendingIncoming" })).toBe("Request received");
    expect(rowNote({ relation: "pendingOutgoing" })).toBe("Request sent");
    expect(rowNote({ relation: "ignored" })).toBe("Ignored");
    expect(rowNote({ relation: "contact" })).toBeNull();
  });

  it("AND-090 offers Android's four orders, by name first", () => {
    expect(SORTS.map((s) => s.id)).toEqual(["displayName", "username", "dateAdded", "lastActivity"]);
    expect(SORTS.map((s) => s.title)).toEqual(["Name", "Username", "Date added", "Last activity"]);
  });

  it("IOS-072 searches the network from two characters, latest prefix trimmed", () => {
    expect(networkPrefix("")).toBeNull();
    expect(networkPrefix(" a ")).toBeNull();
    expect(networkPrefix(" al ")).toBe("al");
    // Two characters, not two UTF-16 units.
    expect(networkPrefix("𝒜")).toBeNull();
  });

  it("IOS-072 shows at most three network hits, strangers only: the identity and its relations are listed above", () => {
    const hits = [hit(0, "isSelf"), hit(1, "contact"), hit(2, "ignored"), hit(3), hit(4), hit(5), hit(6, "pendingIncoming"), hit(7)];
    expect(networkHits(hits, undefined)).toEqual({ shown: [hit(3), hit(4), hit(5)], more: true });
    expect(networkHits(hits.slice(0, 5), undefined)).toEqual({ shown: [hit(3), hit(4)], more: false });
    expect(networkHits([], undefined)).toEqual({ shown: [], more: false });
    expect(networkHits(undefined, undefined)).toEqual({ shown: [], more: false });
  });

  it("IOS-072 drops a cached stranger hit the contact list now holds (D21)", () => {
    // Hit 3 was read as a stranger; since then its request arrived and the list holds it.
    const hits = [hit(3), hit(4), hit(5), hit(7)];
    const listed = list({ requests: [summary(3, { relation: "pendingIncoming" })] });
    expect(networkHits(hits, listed)).toEqual({ shown: [hit(4), hit(5), hit(7)], more: false });
  });

  it("IOS-072 names the relation and the payment channel in the detail slot", () => {
    expect(relationTitle("pendingOutgoing")).toBe("Request sent");
    expect(relationTitle("stranger")).toBe("Not a contact");
    expect(channelTitle("setupPending")).toBe("Setting up");
    expect(channelTitle("broken")).toBe("Unavailable");
  });

  it("finds the selected contact in any group, and nothing for an id the list does not hold", () => {
    const l = list({ requests: [summary(1)], hidden: [summary(2)] });
    expect(findContact(l, ID(2))).toEqual(summary(2));
    expect(findContact(l, ID(3))).toBeNull();
    expect(findContact(l, null)).toBeNull();
    expect(findContact(undefined, ID(1))).toBeNull();
  });

  it("IOS-072 says why contacts still set up: unlock when unlocked for mixing only, finishing when unlocked", () => {
    expect(setupHint(1, "unlockedMixingOnly")).toEqual({ kind: "unlock", text: "Unlock to finish setting up 1 contact" });
    expect(setupHint(3, "unlocked")).toEqual({ kind: "finishing", text: "Finishing the setup of 3 contacts…" });
    expect(setupHint(2, "unencrypted")?.kind).toBe("finishing");
    expect(setupHint(0, "unlockedMixingOnly")).toBeNull();
    expect(setupHint(undefined, "unlocked")).toBeNull();
    expect(setupHint(2, undefined)).toBeNull();
    // The lock screen covers Contacts then.
    expect(setupHint(2, "locked")).toBeNull();
  });

  it("moves the keyboard focus down, up, to the ends, and stays in bounds", () => {
    const ids = ["a", "b", "c"];
    expect(moveFocus(ids, null, "ArrowDown")).toBe("a");
    expect(moveFocus(ids, "a", "ArrowDown")).toBe("b");
    expect(moveFocus(ids, "c", "ArrowDown")).toBe("c");
    expect(moveFocus(ids, "b", "ArrowUp")).toBe("a");
    expect(moveFocus(ids, "a", "ArrowUp")).toBe("a");
    expect(moveFocus(ids, "b", "End")).toBe("c");
    expect(moveFocus(ids, "c", "Home")).toBe("a");
    expect(moveFocus(ids, "a", "Enter")).toBeNull();
    expect(moveFocus([], null, "ArrowDown")).toBeNull();
  });
});
