import { describe, expect, it } from "vitest";
import { EMPTY_ARTS } from "./emptyArt";
import { addressBookEmpty, activityFiltersApplied, EMPTY } from "./emptyStates";

const WITH_ACTION = ["homeHistory", "activity", "activityFiltered", "contacts", "contactRequests", "notificationsNoIdentity", "addressSend", "addressReceive", "addressSearch", "receiveRequests", "coinjoin", "coins", "coinsHidden", "peers", "payContacts", "join", "wallet"] as const;

describe("empty states", () => {
  it("gives every screen a title, a sentence and a known drawing", () => {
    for (const copy of Object.values(EMPTY)) {
      expect(copy.title.length).toBeGreaterThan(0);
      expect(copy.message.length).toBeGreaterThan(0);
      expect(EMPTY_ARTS).toContain(copy.art);
      expect(copy.action === null || copy.action.label.length > 0).toBe(true);
    }
  });

  it("gives the no-data screens one action, and leaves a caught-up inbox without one", () => {
    for (const id of WITH_ACTION) expect(EMPTY[id].action).not.toBeNull();
    expect(EMPTY.notifications.action).toBeNull();
    const ids = Object.values(EMPTY).flatMap((c) => (c.action ? [c.action.testId] : []));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("draws the headline empty screens differently", () => {
    const arts = [EMPTY.activity, EMPTY.contacts, EMPTY.contactRequests, EMPTY.addressSend, EMPTY.receiveRequests, EMPTY.coinjoin, EMPTY.notifications].map((c) => c.art);
    expect(new Set(arts).size).toBe(arts.length);
  });
});

describe("activityFiltersApplied", () => {
  const open = { categoryId: "all", text: "", date: "all" };

  it("is false for an untouched list", () => {
    expect(activityFiltersApplied(open)).toBe(false);
  });

  it("is true once a category, search, date or minimum amount is applied", () => {
    expect(activityFiltersApplied({ ...open, categoryId: "sent" })).toBe(true);
    expect(activityFiltersApplied({ ...open, text: "rent" })).toBe(true);
    expect(activityFiltersApplied({ ...open, date: "week" })).toBe(true);
    expect(activityFiltersApplied({ ...open, minAmount: 1 })).toBe(true);
  });
});

describe("addressBookEmpty", () => {
  it("is absent when there are rows", () => {
    expect(addressBookEmpty({ search: "", purpose: "send", count: 2 })).toBeNull();
  });

  it("offers add for sending, Receive for receiving, and clear when a search misses", () => {
    expect(addressBookEmpty({ search: "", purpose: "send", count: 0 })?.action?.label).toBe("Add address");
    expect(addressBookEmpty({ search: "", purpose: "receive", count: 0 })?.action?.label).toBe("Receive");
    expect(addressBookEmpty({ search: "zz", purpose: "send", count: 0 })?.art).toBe("search");
  });
});
