// The fixture's `dashpay_contacts` orders and filters as the engine does (m4 §2.4 `contacts`), so
// the Contacts screen's tests (DP2-07) exercise the engine's rules, not the fixture's insertion order.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { ContactQueryArgs } from "../api/generated/ContactQueryArgs";
import type { ContactSortName } from "../api/generated/ContactSortName";
import { isAppError } from "../api/backend";
import { IDENTITY } from "./dashpay";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID, type ScenarioName } from "./scenarios";

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const query = (q: Partial<ContactQueryArgs>): ContactQueryArgs => ({ sections: [], sort: "displayName", text: null, ...q });

async function usernames(scenario: ScenarioName, q: Partial<ContactQueryArgs>) {
  installFixture({ scenario });
  const list = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query(q));
  return Object.fromEntries(list.sections.map((s) => [s.section, s.contacts.map((c) => c.username)]));
}

describe("dashpay_contacts (fixture, as the engine)", () => {
  it("lists every section once, in the engine's order, for an empty section list", async () => {
    installFixture({ scenario: "dashpay-registered" });
    const list = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query({}));
    expect(list.sections.map((s) => s.section)).toEqual(["requests", "contacts", "pending", "hidden"]);
    expect(list.updating).toBe(false);
    const some = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query({ sections: ["hidden", "requests", "hidden"] }));
    expect(some.sections.map((s) => s.section)).toEqual(["hidden", "requests"]);
  });

  // Android's four orders (UsernameSortOrderBy) on My Contacts and Hidden; requests stay newest first.
  const ORDERS: Record<ContactSortName, string[]> = {
    // The shown name: alias, display name, username: "Bob Builder", "Carol", "dave", "Erin".
    displayName: ["bob", "carol", "dave", "erin"],
    username: ["bob", "carol", "dave", "erin"],
    // Carol 2 days ago, Erin 5, Bob 40, Dave 90.
    dateAdded: ["carol", "erin", "bob", "dave"],
    // The last payment, else the contact's own time (the engine's `paid.max(since)`): Bob a day
    // ago, Carol 2 days, Erin 5, Dave 12.
    lastActivity: ["bob", "carol", "erin", "dave"],
  };
  for (const [sort, order] of Object.entries(ORDERS) as [ContactSortName, string[]][]) {
    it(`AND-090 sorts My Contacts by ${sort}`, async () => {
      const sections = await usernames("dashpay-registered", { sort });
      expect(sections.contacts).toEqual(order);
      // Requests and Pending ignore the sort: newest first. Heidi's came an hour after Grace's,
      // the reverse of their name, username and id orders, so only newest first gives this.
      expect(sections.requests).toEqual(["heidi", "grace"]);
      expect(sections.pending).toEqual(["frank"]);
      // Hidden follows the sort too; Ivan has no time at all, so he is last by time.
      expect(sections.hidden).toEqual(sort === "dateAdded" || sort === "lastActivity" ? ["judy", "ivan"] : ["ivan", "judy"]);
    });
  }

  it("AND-090 sorts by the alias before the display name, and ties by id", async () => {
    const fixture = installFixture({ scenario: "dashpay-registered" });
    const bob = fixture.world.dashpay.contacts.find((c) => c.summary.username === "bob")!;
    bob.summary = { ...bob.summary, alias: "Zed" };
    const list = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query({ sort: "displayName" }));
    expect(list.sections[1].contacts.map((c) => c.username)).toEqual(["carol", "dave", "erin", "bob"]);
  });

  it("IOS-072 filters on the username, display name and alias, and from four characters the id's start", async () => {
    expect((await usernames("dashpay-registered", { text: "  builder " })).contacts).toEqual(["bob"]);
    expect((await usernames("dashpay-registered", { text: "WORK" })).hidden).toEqual(["judy"]);
    expect((await usernames("dashpay-registered", { text: "GrAC" })).requests).toEqual(["grace"]);
    // Three characters of an id match nothing by id; four do.
    expect(Object.values(await usernames("dashpay-registered", { text: IDENTITY.dave.slice(0, 3) })).flat()).toEqual([]);
    expect((await usernames("dashpay-registered", { text: IDENTITY.dave.slice(0, 4) })).contacts).toEqual(["dave"]);
    expect(Object.values(await usernames("dashpay-registered", { text: "   " })).flat()).toHaveLength(9);
  });

  it("answers `updating` while a pass holds the wallet: empty sections before any snapshot, else the last one", async () => {
    installFixture({ scenario: "dashpay-contacts-updating" });
    const list = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query({}));
    expect(list.updating).toBe(true);
    expect(list.sections.flatMap((s) => s.contacts)).toEqual([]);
    installFixture({ scenario: "dashpay-contacts-refreshing" });
    const last = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, query({}));
    expect(last.updating).toBe(true);
    expect(last.sections.flatMap((s) => s.contacts)).toHaveLength(9);
  });

  it("refuses a user search with platform.unavailable when DAPI does not answer", async () => {
    installFixture({ scenario: "dashpay-search-offline" });
    const e = await cmd.searchUsers(FIXTURE_WALLET_ID, "ma", 12).catch((e: unknown) => e);
    expect(isAppError(e) && e.code).toBe("platform.unavailable");
  });

  it("finds more than three strangers for “ma”, so the network search has more to show", async () => {
    installFixture({ scenario: "dashpay-registered" });
    const hits = await cmd.searchUsers(FIXTURE_WALLET_ID, " Ma", 12);
    expect(hits.map((h) => h.username)).toEqual(["mallory", "marta", "mateo", "maxine", "mayumi"]);
    expect(hits.every((h) => h.relation === "stranger")).toBe(true);
  });
});
