// The fixture's answers for the contact pane and the Enable DashPay banner (DP2-09 D18): the
// grant bounds, the activity pages, the key upgrade and the private details refuse and charge as
// the engine's calls do (m4 §2.4, flows.rs), so the screen tests exercise the engine's rules.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { PrivateContactDetails } from "../api/generated/PrivateContactDetails";
import { ACTIVITY_PAGE, IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const ME = IDENTITY.alice;
const install = (scenario: ScenarioName = "dashpay-registered"): FixtureHandle => installFixture({ scenario });
const grant = async (maxCredits: string) => (await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits }, W, { kind: "passphrase", passphrase: "demo" })).id;
const details = (d: Partial<PrivateContactDetails>): PrivateContactDetails => ({ alias: null, note: null, hidden: false, ...d });

describe("grant_request (fixture, as flows.rs)", () => {
  it("bounds each write's credits and funds nothing from Core", async () => {
    install();
    expect(await cmd.grantRequest(W, ME, { kind: "acceptRequest" })).toEqual({ maxDuffs: 0, maxCredits: "150000000" });
    expect(await cmd.grantRequest(W, ME, { kind: "publishPrivateDetails" })).toEqual({ maxDuffs: 0, maxCredits: "300000000" });
    expect(await cmd.grantRequest(W, ME, { kind: "enableDashPayKeys" })).toEqual({ maxDuffs: 0, maxCredits: "100000000" });
    await expect(cmd.grantRequest(W, " ", { kind: "acceptRequest" })).rejects.toMatchObject({ code: "invalid_argument" });
  });
});

describe("contact_activity (fixture, as DP3-02)", () => {
  it("IOS-074 pages a contact's payments newest first, 50 at a time, and filters them", async () => {
    install();
    const first = await cmd.contactActivity(W, ME, IDENTITY.bob, null, "all");
    expect(first.items).toHaveLength(ACTIVITY_PAGE);
    const times = first.items.map((i) => i.timestamp!);
    expect([...times].sort((a, b) => b - a)).toEqual(times);
    const rest = await cmd.contactActivity(W, ME, IDENTITY.bob, first.nextCursor, "all");
    expect(rest.nextCursor).toBeNull();
    // 52 older payments, the journal's newest one, and the history row attributed to Bob.
    expect(first.items.length + rest.items.length).toBe(54);
    const sent = await cmd.contactActivity(W, ME, IDENTITY.bob, null, "sent");
    expect(sent.items.every((i) => i.direction === "sent")).toBe(true);
    expect(sent.nextCursor).toBeNull();
  });

  it("refuses a cursor of another filter or contact, the identity itself and another wallet's identity; a contact never paid is empty", async () => {
    install();
    const first = await cmd.contactActivity(W, ME, IDENTITY.bob, null, "all");
    await expect(cmd.contactActivity(W, ME, IDENTITY.bob, first.nextCursor, "sent")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.contactActivity(W, ME, IDENTITY.dave, first.nextCursor, "all")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.contactActivity(W, ME, ME, null, "all")).rejects.toMatchObject({ code: "contact.self" });
    await expect(cmd.contactActivity(W, IDENTITY.bob, ME, null, "all")).rejects.toMatchObject({ code: "identity.not_found" });
    expect(await cmd.contactActivity(W, ME, IDENTITY.carol, null, "all")).toEqual({ items: [], nextCursor: null });
  });
});

describe("dashpay_enable_keys (fixture, as DP2-02)", () => {
  it("IOS-072 adds the keys under a grant that covers the quote, once", async () => {
    install("dashpay-no-keys");
    await expect(cmd.dashpayEnableKeys(W, ME, await grant("1000"))).rejects.toMatchObject({ code: "platform.grant_exceeded" });
    await cmd.dashpayEnableKeys(W, ME, await grant("100000000"));
    expect((await cmd.identities(W))[0].hasDashpayKeys).toBe(true);
    // Nothing missing: nothing sent, the grant left unused.
    const unused = await grant("100000000");
    await cmd.dashpayEnableKeys(W, ME, unused);
    expect((await cmd.leases()).filter((l) => l.flow === "enableDashPayKeys")).toHaveLength(1);
  });

  it("refuses an identity with too few credits before the grant", async () => {
    const f = install("dashpay-no-keys");
    f.world.dashpay.identities[0].balance = "1000";
    await expect(cmd.dashpayEnableKeys(W, ME, await grant("100000000"))).rejects.toMatchObject({ code: "platform.insufficient_credits" });
  });
});

describe("dashpay_set_private_details (fixture, as DP2-04)", () => {
  it("IOS-074 publishes under a grant, trims, and needs one to change published details", async () => {
    install();
    const bob = IDENTITY.bob;
    expect(await cmd.dashpaySetPrivateDetails(W, ME, bob, details({ alias: "  Bobby ", note: " " }), await grant("300000000"))).toBe("published");
    const c = await cmd.dashpayContact(W, ME, bob);
    expect(c?.privateDetails).toEqual({ alias: "Bobby", note: null, hidden: false });
    expect(c?.summary.alias).toBe("Bobby");
    await expect(cmd.dashpaySetPrivateDetails(W, ME, bob, details({ alias: "B" }), null)).rejects.toMatchObject({ code: "platform.needs_grant" });
    // The same details again: published, nothing sent.
    expect(await cmd.dashpaySetPrivateDetails(W, ME, bob, details({ alias: "Bobby" }), null)).toBe("published");
  });

  it("saves locally without a grant, defers below two contacts, and refuses non-contacts and long details", async () => {
    const f = install();
    expect(await cmd.dashpaySetPrivateDetails(W, ME, IDENTITY.carol, details({ hidden: true }), null)).toBe("local");
    await expect(cmd.dashpaySetPrivateDetails(W, ME, IDENTITY.grace, details({}), null)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.dashpaySetPrivateDetails(W, ME, ME, details({}), null)).rejects.toMatchObject({ code: "contact.self" });
    await expect(cmd.dashpaySetPrivateDetails(W, ME, IDENTITY.bob, details({ note: "x".repeat(2100) }), null)).rejects.toMatchObject({
      code: "contact.details_too_long",
      data: { kind: "size", max: 2031 },
    });
    f.world.dashpay.contacts = f.world.dashpay.contacts.filter((c) => c.summary.relation !== "contact" || c.summary.contact === IDENTITY.bob);
    expect(await cmd.dashpaySetPrivateDetails(W, ME, IDENTITY.bob, details({ alias: "B" }), await grant("300000000"))).toBe("deferredUntilTwoContacts");
  });
});
