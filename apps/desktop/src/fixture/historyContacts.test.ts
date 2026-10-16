// The `dashpay-history-contacts` scenario (DP3-07 D10), through the generated commands as Home,
// Activity and the detail sheet read it: each D1–D4 branch has a row among the newest, and the
// detail carries the record's counterparty.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { findContact } from "../contacts/model";
import { contactLetters, txTitle } from "../format/tx";
import { IDENTITY } from "./dashpay";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID as W } from "./scenarios";

const ME = IDENTITY.alice;

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  installFixture({ scenario: "dashpay-history-contacts" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const page = () => cmd.historyPage(W, { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 200 });

describe("fixture history contacts", () => {
  it("IOS-029 puts a stranger, a labelled contact payment and Bob and Dave among the rows", async () => {
    const { rows } = await page();
    const [marta, refund] = rows;
    expect(marta.counterparty?.identity).toBe(IDENTITY.marta);
    expect([txTitle(marta), contactLetters(marta.counterparty)]).toEqual(["Marta", "M"]);
    expect(refund.counterparty?.identity).toBe(IDENTITY.bob);
    expect([txTitle(refund), contactLetters(refund.counterparty)]).toEqual(["Coffee refund", "BB"]);
    const named = rows.filter((r) => r.counterparty).map((r) => [txTitle(r), contactLetters(r.counterparty)]);
    expect(named).toEqual(expect.arrayContaining([["Bob Builder", "BB"], ["@dave", "D"], ["Received", null], ["Judy", "J"]]));
    expect(named).toHaveLength(6);
  });

  it("lists Bob, Dave and hidden Judy in the main identity's contacts, not Marta nor the nameless one (D4)", async () => {
    const list = await cmd.dashpayContacts(W, ME, { sections: [], sort: "displayName", text: null });
    const listed = (id: string) => findContact(list, id) !== null;
    expect([IDENTITY.bob, IDENTITY.dave, IDENTITY.judy].map(listed)).toEqual([true, true, true]);
    expect(list.sections.find((s) => s.section === "hidden")?.contacts.map((c) => c.contact)).toContain(IDENTITY.judy);
    expect([IDENTITY.marta, IDENTITY.mateo].map(listed)).toEqual([false, false]);
  });

  it("gives the detail the row's counterparty, and Bob's activity the refund", async () => {
    const { rows } = await page();
    const d = await cmd.txDetail(W, rows[1].txid);
    expect(d.rows[0].counterparty).toEqual({ identity: IDENTITY.bob, username: "bob", displayName: "Bob Builder", avatarUrl: null });
    const activity = await cmd.contactActivity(W, ME, IDENTITY.bob, null, "all");
    expect(activity.items.map((i) => i.txid)).toContain(rows[1].txid);
  });
});
