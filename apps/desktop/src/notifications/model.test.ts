// The bell's rules (DP2-09 D11–D13), by parity ID.

import { describe, expect, it } from "vitest";
import type { ContactList } from "../api/generated/ContactList";
import type { DashPayEventInfo } from "../api/generated/DashPayEventInfo";
import type { DashPayEventPage } from "../api/generated/DashPayEventPage";
import { eventLink, eventText, feedOf, namesFrom, newestId, popoverCut } from "./model";

const BOB = "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN";
const UNKNOWN = "MALLoRyCRFtBn7r9fqD4ZcVaxu3Rx1oEkJ38EN6Hbnm";

const list: ContactList = {
  updating: false,
  sections: [
    {
      section: "contacts",
      contacts: [
        {
          contact: BOB,
          username: "bob",
          displayName: "Bob Builder",
          alias: null,
          relation: "contact",
          hidden: false,
          channel: "ready",
          since: 1,
          lastActivityAt: null,
          unverified: false,
        },
      ],
    },
  ],
};

const ev = (id: number, patch: Partial<DashPayEventInfo> = {}): DashPayEventInfo => ({
  id,
  kind: "requestAccepted",
  contact: BOB,
  reference: "1",
  at: id,
  readAt: null,
  ...patch,
});

describe("event lines", () => {
  const names = namesFrom(list);

  it("IOS-075 names the contact as the list shows it, else its short id", () => {
    expect(eventText(ev(1, { kind: "requestReceived" }), names)).toBe("Bob Builder sent you a contact request");
    expect(eventText(ev(1, { kind: "requestAccepted", contact: UNKNOWN }), names)).toBe("MALLoR…Hbnm accepted your contact request");
    expect(eventText(ev(1, { kind: "contactEstablished" }), names)).toBe("You and Bob Builder are now contacts");
    expect(eventText(ev(1, { kind: "paymentReceived", reference: "b0" }), names)).toBe("Bob Builder sent you a payment");
  });

  it("says the username events with their label", () => {
    const named = (kind: DashPayEventInfo["kind"]) => eventText(ev(1, { kind, contact: null, reference: "alice" }), names);
    expect(named("usernameRegistered")).toBe("Your username @alice is registered");
    expect(named("contestWon")).toBe("You won the vote for @alice");
    expect(named("contestLost")).toBe("You lost the vote for @alice");
    expect(named("contestLocked")).toBe("The vote for @alice ended locked");
  });

  it("leads a contact event to the contact and a payment to its transaction", () => {
    expect(eventLink(ev(1, { kind: "requestReceived" }))).toEqual({ kind: "contact", contact: BOB });
    expect(eventLink(ev(1, { kind: "paymentReceived", reference: "b0" }))).toEqual({ kind: "tx", txid: "b0" });
    expect(eventLink(ev(1, { kind: "contestWon", contact: null }))).toBeNull();
  });
});

describe("the feed of an opening", () => {
  const page = (p: Partial<DashPayEventPage>): DashPayEventPage => ({ pending: [], new: [], earlier: [], nextCursor: null, ...p });

  it("IOS-075 keeps what was unread at the opening under New after it was marked read", () => {
    const opened = page({ new: [ev(3), ev(2)], earlier: [ev(1, { readAt: 5 })] });
    expect(newestId(opened)).toBe(3);
    const unreadAtOpen = new Set([3, 2]);
    // The re-read after mark_read: everything is read now.
    const reread = page({ earlier: [ev(3, { readAt: 9 }), ev(2, { readAt: 9 }), ev(1, { readAt: 5 })] });
    const feed = feedOf([reread], unreadAtOpen);
    expect(feed.fresh.map((e) => e.id)).toEqual([3, 2]);
    expect(feed.earlier.map((e) => e.id)).toEqual([1]);
    // An event arriving while open is new too.
    expect(feedOf([page({ new: [ev(4)], earlier: reread.earlier })], unreadAtOpen).fresh.map((e) => e.id)).toEqual([4, 3, 2]);
    expect(newestId(page({}))).toBeNull();
  });

  it("cuts the popover at five rows and counts the rest", () => {
    const pending = list.sections[0].contacts;
    const feed = { pending: Array(7).fill(pending[0]), fresh: [1, 2, 3, 4, 5, 6].map((i) => ev(i)), earlier: [] };
    const cut = popoverCut(feed);
    expect([cut.pending.length, cut.morePending, cut.fresh.length, cut.moreFresh]).toEqual([5, 2, 5, 1]);
  });
});
