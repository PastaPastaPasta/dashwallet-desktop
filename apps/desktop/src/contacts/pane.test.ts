// The contact pane's rules (DP2-09 D2, D7, D8, D9, D10, D14), by parity ID.

import { describe, expect, it } from "vitest";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { CostTable } from "../api/generated/CostTable";
import { activityRow, creditsToDuffs, detailsChanged, detailsForm, detailsOf, enableBanner, ENABLE_FIRST, paneActions, selectionAnnouncement } from "./pane";

const row = (patch: Partial<ContactSummaryInfo>): ContactSummaryInfo => ({
  contact: "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ",
  username: "grace",
  displayName: "Grace Hopper",
  alias: null,
  relation: "contact",
  hidden: false,
  channel: "ready",
  since: 1,
  lastActivityAt: null,
  unverified: false,
  ...patch,
});

describe("paneActions", () => {
  it("IOS-074 offers Accept and Ignore for a request, held back without DashPay keys", () => {
    expect(paneActions(row({ relation: "pendingIncoming" }), true)).toEqual({ kind: "request", acceptBlocked: null });
    expect(paneActions(row({ relation: "pendingIncoming" }), undefined)).toEqual({ kind: "request", acceptBlocked: null });
    expect(paneActions(row({ relation: "pendingIncoming" }), false)).toEqual({ kind: "request", acceptBlocked: ENABLE_FIRST });
  });

  it("IOS-074 offers Pay to a contact whose channel is ready, and says why not otherwise", () => {
    expect(paneActions(row({}), true)).toEqual({ kind: "contact", payBlocked: null });
    expect(paneActions(row({ channel: "setupPending" }), true)).toEqual({ kind: "contact", payBlocked: "Payments with this contact are still being set up." });
    expect(paneActions(row({ channel: "broken" }), true)).toEqual({ kind: "contact", payBlocked: "Payments to this contact cannot be sent right now." });
    expect(paneActions(row({ channel: "notEstablished" }), true)).toMatchObject({ kind: "contact" });
  });

  it("names a sent request and an ignored sender", () => {
    expect(paneActions(row({ relation: "pendingOutgoing" }), true)).toEqual({ kind: "sent" });
    expect(paneActions(row({ relation: "ignored" }), true)).toEqual({ kind: "ignored" });
    expect(paneActions(row({ relation: "stranger" }), true)).toEqual({ kind: "none" });
  });
});

describe("private details form", () => {
  it("IOS-074 trims, saves blank as none and tells a change", () => {
    const saved = { alias: "Gracie", note: null, hidden: false };
    const form = detailsForm(saved);
    expect(form).toEqual({ alias: "Gracie", note: "", hidden: false });
    expect(detailsChanged(form, saved)).toBe(false);
    expect(detailsChanged({ ...form, alias: " Gracie " }, saved)).toBe(false);
    expect(detailsChanged({ ...form, note: "x" }, saved)).toBe(true);
    expect(detailsOf({ alias: "  ", note: " n ", hidden: true })).toEqual({ alias: null, note: "n", hidden: true });
    expect(detailsForm(undefined)).toEqual({ alias: "", note: "", hidden: false });
  });
});

describe("enableBanner", () => {
  const costs = { enableDashpayKeys: "61330100", creditsPerDuff: 1000 } as CostTable;

  it("IOS-072 shows for the main identity without DashPay keys, with the fee in duffs", () => {
    expect(enableBanner("A", false, costs)).toEqual({ identity: "A", fee: 61331 });
    expect(enableBanner("A", false, undefined)).toEqual({ identity: "A", fee: null });
  });

  it("hides with the keys, or while they are unknown", () => {
    expect(enableBanner("A", true, costs)).toBeNull();
    expect(enableBanner("A", undefined, costs)).toBeNull();
  });

  it("rounds credits up to whole duffs", () => {
    expect(creditsToDuffs("1000", 1000)).toBe(1);
    expect(creditsToDuffs("1001", 1000)).toBe(2);
    expect(creditsToDuffs("5", 0)).toBe(5);
  });
});

describe("activity and announcements", () => {
  it("AND-093 signs sent payments and marks unmined ones", () => {
    expect(activityRow({ txid: "t", direction: "sent", amount: 5, height: null, timestamp: 1 })).toEqual({ title: "Sent", signed: -5, pending: true });
    expect(activityRow({ txid: "t", direction: "received", amount: 5, height: 9, timestamp: 1 })).toEqual({ title: "Received", signed: 5, pending: false });
  });

  it("D14 announces the selection by name and relation", () => {
    expect(selectionAnnouncement(row({ relation: "pendingIncoming" }))).toBe("Grace Hopper, Request received");
    expect(selectionAnnouncement(null)).toBe("");
  });
});
