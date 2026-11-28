// Pay that never dead-ends: the tab Pay opens on, the picker's username search and its "nothing
// matches", and the card of a user that search found.

import { describe, expect, it } from "vitest";
import type { ContactList } from "../../api/generated/ContactList";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { UserHit } from "../../api/generated/UserHit";
import { foundLine, pickerGroups, pickerHits, pickerListing, pickerNetworkState, pickerView, searchAnnouncement } from "./contact";

const BOB = "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN";

const summary = (over: Partial<ContactSummaryInfo> = {}): ContactSummaryInfo => ({
  contact: BOB,
  username: "bob",
  displayName: "Bob Builder",
  alias: null,
  relation: "contact",
  hidden: false,
  channel: "ready",
  since: 1,
  lastActivityAt: 1,
  unverified: false,
  ...over,
});
const list = (contacts: ContactSummaryInfo[], requests: ContactSummaryInfo[] = []): ContactList => ({
  updating: false,
  sections: [
    { section: "contacts", contacts },
    { section: "requests", contacts: requests },
  ],
});
describe("the picker under a search", () => {
  const view = (over: Partial<Parameters<typeof pickerView>[0]>) =>
    pickerView({ text: "ma", groups: 0, anyContacts: true, network: true, networkState: "done", hits: 0, ...over });

  it("shows no username search and no 'nothing matches' without a search", () => {
    expect(view({ text: "", network: false })).toEqual({ network: false, none: null });
  });

  it("shows the users found when no contact matches", () => {
    expect(view({ hits: 2 })).toEqual({ network: true, none: null });
  });

  it("shows the search running or failed, and says nothing matches only once it is done", () => {
    expect(view({ networkState: "loading" })).toEqual({ network: true, none: null });
    expect(view({ networkState: "error" })).toEqual({ network: true, none: null });
    expect(view({})).toEqual({ network: false, none: "anyone" });
  });

  it("says nothing matches when the search found only contacts the picker's filter missed", () => {
    expect(view({ hits: 0 })).toEqual({ network: false, none: "anyone" });
  });

  it("keeps matching contacts, without an empty username section", () => {
    expect(view({ groups: 1 })).toEqual({ network: false, none: null });
    expect(view({ groups: 1, hits: 1 })).toEqual({ network: true, none: null });
  });

  it("under two characters says only that no contact matches", () => {
    expect(view({ text: "m", network: false })).toEqual({ network: false, none: "contacts" });
  });

  it("under two characters without any contact asks to keep typing", () => {
    expect(view({ text: "m", network: false, anyContacts: false })).toEqual({ network: false, none: "short" });
  });

  it("announces what the username search found", () => {
    const say = (over: Partial<Parameters<typeof searchAnnouncement>[0]>) => searchAnnouncement({ network: true, networkState: "done", hits: 0, none: null, ...over });
    expect(say({ none: "anyone" })).toBe("No one found");
    expect(say({ hits: 1 })).toBe("1 person found on DashPay");
    expect(say({ hits: 3 })).toBe("3 people found on DashPay");
    expect(say({ networkState: "error" })).toBe("Could not search");
    expect(say({ networkState: "loading" })).toBeNull();
    expect(say({ network: false })).toBeNull();
    expect(say({})).toBeNull();
  });
});

describe("the username search under the picker", () => {
  const MARTA = "6ce1fd5DbkBSu5PvJp1jR3P5LxcAuFFRtnLsmKnzDNDM";
  const hit = (over: Partial<UserHit> = {}): UserHit => ({
    identity: BOB,
    username: "b0b",
    displayName: "Bob Builder",
    avatarUrl: null,
    relation: "contact",
    unverified: false,
    ...over,
  });

  it("lists a contact the picker's filter missed, and says it found someone", () => {
    // "b0b" finds Bob (the search folds 0 and o); the picker's filter lists no one for it.
    const filtered = list([]);
    const hits = pickerHits([hit()], filtered);
    expect(hits.shown.map((h) => h.identity)).toEqual([BOB]);
    const view = pickerView({ text: "b0b", groups: pickerGroups(filtered).length, anyContacts: true, network: true, networkState: "done", hits: hits.shown.length });
    expect(view).toEqual({ network: true, none: null });
  });

  it("shows three users found and counts them all, for the announcement", () => {
    const found = ["a1", "a2", "a3", "a4", "a5"].map((u, i) => hit({ identity: `${BOB.slice(0, -1)}${i}`, username: u, relation: "stranger" }));
    const hits = pickerHits(found, list([]));
    expect(hits.shown).toHaveLength(3);
    expect(hits).toMatchObject({ more: true, count: 5 });
  });

  it("leaves out only who the picker lists for the text, in its contacts and requests", () => {
    const filtered = list([summary()]);
    const sent = summary({ contact: MARTA, username: "marta", relation: "pendingOutgoing" });
    const withSent: ContactList = { ...filtered, sections: [...filtered.sections, { section: "pending", contacts: [sent] }] };
    const hits = pickerHits([hit({ username: "bob" }), hit({ identity: MARTA, username: "marta", relation: "pendingOutgoing" })], withSent);
    expect(hits.shown.map((h) => h.identity)).toEqual([MARTA]);
  });

  it("runs until the picker's list for the text is read, and is no 'No one found' meanwhile", () => {
    const state = (over: Partial<Parameters<typeof pickerNetworkState>[0]> = {}) =>
      pickerNetworkState({ listReady: true, caughtUp: true, pending: false, failed: false, fetching: false, ...over });
    expect(state()).toBe("done");
    expect(state({ listReady: false })).toBe("loading");
    expect(state({ listReady: false, failed: true })).toBe("loading");
    expect(state({ caughtUp: false })).toBe("loading");
    expect(state({ failed: true })).toBe("error");
    expect(state({ failed: true, fetching: true })).toBe("loading");
    // A re-read of an answered search: its relations may have changed.
    expect(state({ fetching: true })).toBe("loading");
    const networkState = state({ listReady: false });
    expect(pickerView({ text: "zzz", groups: 0, anyContacts: true, network: true, networkState, hits: 0 })).toEqual({ network: true, none: null });
  });
});

describe("the list the username search's hits are matched against", () => {
  const read: ContactList = { sections: [], updating: false };

  it("is the picker's list once read, and none while the previous text's is kept", () => {
    expect(pickerListing({ data: read, placeholder: false, failed: false })).toBe(read);
    expect(pickerListing({ data: read, placeholder: true, failed: false })).toBeNull();
    expect(pickerListing({ data: undefined, placeholder: false, failed: false })).toBeNull();
  });

  it("lists no one when the read failed with nothing kept, so the search still answers", () => {
    expect(pickerListing({ data: undefined, placeholder: false, failed: true })).toEqual({ sections: [], updating: false });
    // A failed refetch keeps the list the picker still shows.
    expect(pickerListing({ data: read, placeholder: false, failed: true })).toBe(read);
  });
});

describe("the card of a user the search found", () => {
  const line = (data: Parameters<typeof foundLine>[2]["data"], failed = false) => foundLine("Marta", "stranger", { data, failed });

  it("offers a contact request, and says when they can be paid", () => {
    expect(line("ok")).toEqual({ text: "You can pay Marta once they accept your contact request.", next: "send" });
    expect(line("pendingOutgoing")).toEqual({ text: "Contact request sent. You can pay Marta once they accept.", next: null });
  });

  it("pays a contact, and accepts and pays a request", () => {
    expect(line("alreadyContact")?.next).toBe("pay");
    expect(line("pendingIncoming")?.next).toBe("acceptAndPay");
  });

  it("says why no request can go, with the step that helps", () => {
    expect(line("noDashPayKeys")?.next).toBe("enable");
    expect(line("contactCannotReceive")).toEqual({ text: "This user cannot receive contact requests yet.", next: null });
    expect(foundLine("Alice", "isSelf", { data: undefined, failed: false })).toEqual({ text: "This is you.", next: null });
    expect(line(undefined, true)?.next).toBe("retry");
  });

  it("is checking while the eligibility read runs", () => {
    expect(line(undefined)).toBeUndefined();
  });
});
