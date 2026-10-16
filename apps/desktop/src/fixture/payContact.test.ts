// The fixture's contact payments (DP3-01, DP3-02; DP3-06 D8–D11), through the generated commands
// as the Pay tab calls them: the contact draft and its refusals in the engine's order, the
// reserved address reused until handed out, "Accept and pay" under one lease, the payment lock
// and its resolve, the history's attribution and the frequent strip.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { ContactRecipient } from "../api/generated/ContactRecipient";
import { IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";
import { contactAddress } from "./send";

const ME = IDENTITY.alice;
const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const to = (name: keyof typeof IDENTITY, amount = 1_000_000, extra: Partial<ContactRecipient> = {}): ContactRecipient => ({
  identity: ME,
  contact: IDENTITY[name],
  amount,
  subtractFee: false,
  note: null,
  ...extra,
});

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "dashpay-registered" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

async function draftFor(r: ContactRecipient) {
  const d = await cmd.newTxDraft(W);
  await cmd.draftSetContactRecipient(d, r);
  return d;
}

async function prepared(r: ContactRecipient) {
  const d = await draftFor(r);
  const est = await cmd.draftEstimate(d);
  const [grant] = await cmd.authorizeSet([{ kind: "spend", maxDuffs: est.totalSent + est.fee }], W, PASS);
  return { d, est, p: await cmd.draftPrepare(d, grant.id) };
}

const refusal = (p: Promise<unknown>) => p.then(() => null, (e: cmd.AppError) => ({ code: e.code, data: e.data }));

describe("fixture contact payment", () => {
  it("pays the contact's reserved address, not the wallet's, and attributes it", async () => {
    const { d, est, p } = await prepared(to("bob", 1_000_000, { note: "Lunch" }));
    const [out, change] = p.summary.outputs;
    expect(out).toMatchObject({ address: contactAddress(W, to("bob"), 0), amount: 1_000_000, isMine: false, isChange: false });
    expect(change).toMatchObject({ isChange: true, isMine: true });
    expect(p.summary).toMatchObject({ totalSent: 1_000_000, fee: est.fee, totalDebit: 1_000_000 + est.fee });
    expect(await cmd.draftBroadcast(d, p.handle)).toEqual({ txid: p.summary.txid });
    const row = fixture.world.wallets[0].rows[0];
    expect(row).toMatchObject({ txid: p.summary.txid, counterparty: { identity: IDENTITY.bob, username: "bob", displayName: "Bob Builder" } });
    const activity = await cmd.contactActivity(W, ME, IDENTITY.bob, null, "sent");
    expect(activity.items[0]).toMatchObject({ txid: p.summary.txid, direction: "sent", amount: 1_000_000 });
  });

  it("puts a new payee on the frequent strip by count, then the latest payment", async () => {
    const names = async () => (await cmd.frequentContacts(W, ME, 8)).map((c) => c.username);
    expect(await names()).toEqual(["bob", "dave"]);
    const { d, p } = await prepared(to("carol"));
    await cmd.draftBroadcast(d, p.handle);
    // Carol and Dave were paid once each, Carol last; Bob many times.
    expect(await names()).toEqual(["bob", "carol", "dave"]);
    expect(await cmd.frequentContacts(W, ME, 1)).toHaveLength(1);
  });

  it("keeps the reserved address until a broadcast hands it out", async () => {
    const first = await prepared(to("bob"));
    fixture.world.send.broadcast = "noPeers";
    expect(await refusal(cmd.draftBroadcast(first.d, first.p.handle))).toMatchObject({ code: "send.no_peers" });
    const again = await prepared(to("bob"));
    expect(again.p.summary.outputs[0].address).toBe(contactAddress(W, to("bob"), 0));
    fixture.world.send.broadcast = "ok";
    await cmd.draftBroadcast(again.d, again.p.handle);
    const next = await prepared(to("bob"));
    expect(next.p.summary.outputs[0].address).toBe(contactAddress(W, to("bob"), 1));
    expect(contactAddress(W, to("bob"), 1)).not.toBe(contactAddress(W, to("bob"), 0));
  });

  it("refuses as DP3-01 does", async () => {
    const draft = async (r: ContactRecipient) => refusal(cmd.newTxDraft(W).then((d) => cmd.draftSetContactRecipient(d, r)));
    expect(await draft({ ...to("bob"), contact: ME })).toMatchObject({ code: "contact.self" });
    expect(await draft({ ...to("bob"), contact: "nope" })).toMatchObject({ code: "invalid_argument" });
    expect(await draft(to("frank"))).toMatchObject({ code: "invalid_argument" });
    expect(await draft(to("ivan"))).toMatchObject({ code: "invalid_argument" });
    expect(await draft(to("dave"))).toMatchObject({ code: "contact.channel_broken" });
    expect(await draft(to("bob", 100))).toMatchObject({ code: "send.dust_amount" });
    expect(await draft(to("bob", 0))).toMatchObject({ code: "send.invalid_amount" });
    // The amount before the payee, as `validate_recipients` runs first.
    expect(await draft(to("dave", 100))).toMatchObject({ code: "send.dust_amount" });
    expect(await draft(to("frank", 0))).toMatchObject({ code: "send.invalid_amount" });
    // A request may be drafted (Accept and pay) but not prepared before the accept.
    const grace = await draftFor(to("grace"));
    const [grant] = await cmd.authorizeSet([{ kind: "spend", maxDuffs: 10_000_000 }], W, PASS);
    expect(await refusal(cmd.draftPrepare(grace, grant.id))).toEqual({ code: "contact.ineligible", data: { kind: "ineligible", reason: "pendingIncoming" } });
    // A vault grant too small is exceeded, as for an address.
    const bob = await draftFor(to("bob"));
    const [small] = await cmd.authorizeSet([{ kind: "spend", maxDuffs: 1000 }], W, PASS);
    expect(await refusal(cmd.draftPrepare(bob, small.id))).toMatchObject({ code: "send.grant_exceeded", data: { kind: "spendCap", maxDuffs: 1000 } });
  });

  it("accepts and pays under one lease, charged once", async () => {
    const d = await draftFor(to("grace"));
    const est = await cmd.draftEstimate(d);
    const quote = await cmd.grantRequest(W, ME, { kind: "acceptRequest" });
    const grants = await cmd.authorizeSet(
      [
        { kind: "platformOp", maxDuffs: quote.maxDuffs, maxCredits: quote.maxCredits },
        { kind: "spend", maxDuffs: est.totalSent + est.fee },
      ],
      W,
      PASS,
    );
    const lease = await cmd.beginFlow(W, "acceptAndPay", grants.map((g) => g.id));
    expect(await cmd.dashpayAcceptRequest(W, ME, IDENTITY.grace, lease)).toMatchObject({ kind: "established" });
    const p = await cmd.draftPrepare(d, lease);
    expect(p.summary.outputs[0]).toMatchObject({ isMine: false, amount: 1_000_000 });
    // The budget is spent: a second prepare under the lease is over it.
    expect(await refusal(cmd.draftPrepare(d, lease))).toMatchObject({ code: "send.grant_exceeded", data: { kind: "spendCap", maxDuffs: 0 } });
    expect(await refusal(cmd.dashpayAcceptRequest(W, ME, IDENTITY.grace, lease))).toMatchObject({ code: "contact.already_contact" });
    await cmd.draftBroadcast(d, p.handle);
    await cmd.endFlow(lease);
    expect(await refusal(cmd.draftPrepare(d, lease))).toMatchObject({ code: "send.grant_invalid" });
  });

  it("refuses a lease of another flow, and a revoked one is cancelled", async () => {
    const d = await draftFor(to("bob"));
    const quote = await cmd.grantRequest(W, ME, { kind: "acceptRequest" });
    const [op] = await cmd.authorizeSet([{ kind: "platformOp", maxDuffs: quote.maxDuffs, maxCredits: quote.maxCredits }], W, PASS);
    const accept = await cmd.beginFlow(W, "accept", [op.id]);
    expect(await refusal(cmd.draftPrepare(d, accept))).toMatchObject({ code: "send.grant_invalid" });
    const [spend] = await cmd.authorizeSet([{ kind: "spend", maxDuffs: 10_000_000 }], W, PASS);
    const pay = await cmd.beginFlow(W, "contactPayment", [spend.id]);
    await cmd.lock();
    await cmd.unlock(FIXTURE_PASSPHRASE);
    expect(await refusal(cmd.draftPrepare(d, pay))).toMatchObject({ code: "send.cancelled" });
  });

  it("locks the contact on an unknown outcome until the payment resolves", async () => {
    const first = await prepared(to("bob"));
    const second = await prepared(to("bob"));
    fixture.world.send.broadcast = "unknown";
    expect(await refusal(cmd.draftBroadcast(first.d, first.p.handle))).toMatchObject({ code: "send.broadcast_unknown" });
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.bob)).toMatchObject({ txid: first.p.summary.txid });
    const locked = { code: "contact.payment_locked", data: { kind: "txid", txid: first.p.summary.txid } };
    expect(await refusal(cmd.draftBroadcast(second.d, second.p.handle))).toEqual(locked);
    expect(await refusal(draftFor(to("bob")))).toEqual(locked);
    // The same transaction may be broadcast again; going out clears the lock.
    fixture.world.send.broadcast = "ok";
    expect(await cmd.draftBroadcast(first.d, first.p.handle)).toEqual({ txid: first.p.summary.txid });
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.bob)).toBeNull();
  });

  it("resolves a lock: sent when handed over, else the evidence; no lock is invalid", async () => {
    const p = await prepared(to("bob"));
    fixture.world.send.broadcast = "unknown";
    await refusal(cmd.draftBroadcast(p.d, p.p.handle));
    expect(await cmd.resolveContactPaymentLock(W, ME, IDENTITY.bob)).toBe("sent");
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.bob)).toBeNull();
    expect(await refusal(cmd.resolveContactPaymentLock(W, ME, IDENTITY.bob))).toMatchObject({ code: "invalid_argument" });
    // Evidence set answers even for a payment handed over: still unknown, the lock stays.
    const again = await prepared(to("bob"));
    await refusal(cmd.draftBroadcast(again.d, again.p.handle));
    fixture.world.send.lockEvidence = "unknown";
    expect(await cmd.resolveContactPaymentLock(W, ME, IDENTITY.bob)).toBe("unknown");
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.bob)).toMatchObject({ txid: again.p.summary.txid });
    fixture.world.send.lockEvidence = null;
    // Dave's lock is the scenario's: the fixture never saw its transaction.
    expect(await cmd.resolveContactPaymentLock(W, ME, IDENTITY.dave)).toBe("unknown");
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.dave)).not.toBeNull();
    fixture.world.send.lockEvidence = "notSent";
    expect(await cmd.resolveContactPaymentLock(W, ME, IDENTITY.dave)).toBe("notSent");
    expect(await cmd.contactPaymentLock(W, ME, IDENTITY.dave)).toBeNull();
  });
});
