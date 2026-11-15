// Pay that never dead-ends: the tab Pay opens on, the picker's username search and its "nothing
// matches", and the card of a user that search found.

import { describe, expect, it } from "vitest";
import type { ContactList } from "../../api/generated/ContactList";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import { defaultPayTab, foundLine, pickerView, searchAnnouncement } from "./contact";

const ME = "8tvHgfq4BLx3bRBcxSs7ZsjaTnAZDSmLhNpAgXyNomeR";
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
const identity = { kind: "identity", identity: ME } as const;

describe("the tab Pay opens on", () => {
  it("is To a contact when there is a contact or a request to pay", () => {
    expect(defaultPayTab(identity, { data: list([summary()]), failed: false })).toBe("contact");
    expect(defaultPayTab(identity, { data: list([], [summary({ relation: "pendingIncoming" })]), failed: false })).toBe("contact");
  });

  it("is To an address without contacts", () => {
    expect(defaultPayTab(identity, { data: list([]), failed: false })).toBe("address");
    expect(defaultPayTab(identity, { data: { updating: false, sections: [] }, failed: false })).toBe("address");
  });

  it("is To an address without a username, or while DashPay cannot say", () => {
    expect(defaultPayTab({ kind: "join", reason: null }, { data: undefined, failed: false })).toBe("address");
    expect(defaultPayTab({ kind: "join", reason: "watchOnly" }, { data: undefined, failed: false })).toBe("address");
    expect(defaultPayTab({ kind: "registering" }, { data: undefined, failed: false })).toBe("address");
    expect(defaultPayTab({ kind: "starting" }, { data: undefined, failed: false })).toBe("address");
    expect(defaultPayTab({ kind: "error" }, { data: undefined, failed: false })).toBe("address");
  });

  it("waits while DashPay's status is still being read", () => {
    expect(defaultPayTab(undefined, { data: undefined, failed: false })).toBeNull();
  });

  it("waits for the contacts, and pays to an address when they cannot be read", () => {
    expect(defaultPayTab(identity, { data: undefined, failed: false })).toBeNull();
    expect(defaultPayTab(identity, { data: undefined, failed: true })).toBe("address");
  });
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
