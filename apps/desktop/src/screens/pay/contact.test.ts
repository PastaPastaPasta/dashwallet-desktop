// IOS-050: Pay ▸ To a contact's view model (DP3-06): the banner, the picker, "Accept and pay"'s
// steps against a fake backend, and the contact branch of U-03's promise check and refusals.

import { describe, expect, it, vi } from "vitest";
import type { AppError } from "../../api/generated/AppError";
import type { ContactDetailInfo } from "../../api/generated/ContactDetailInfo";
import type { ContactRecipient } from "../../api/generated/ContactRecipient";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { GrantPurposeSpec } from "../../api/generated/GrantPurposeSpec";
import type { PreparedTxSummary } from "../../api/generated/PreparedTxSummary";
import { AcceptFailed, acceptsToo, acceptUnderLease, blocks, noOwner, PAYMENT_LOCK_TEXT, payBanner, payeeOf, pickerGroups, resolveText, type AcceptApi, type BannerInput } from "./contact";
import { broadcastFailure, confirmedOf, isContact, keepsPromise, sendErrorText, subtractsFee, unsignedKeepsPromise } from "./model";

const ME = "8tvHgfq4BLx3bRBcxSs7ZsjaTnAZDSmLhNpAgXyNomeR";
const BOB = "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN";
const WALLET = "ab".repeat(32);
const err = (code: AppError["code"], data: AppError["data"] = null): AppError => ({ code, message: code, data });

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
const detail = (over: Partial<ContactSummaryInfo> = {}, more: Partial<ContactDetailInfo> = {}): ContactDetailInfo => ({
  summary: summary(over),
  profile: null,
  privateDetails: { alias: null, note: null, hidden: false },
  publishState: "local",
  requestSentAt: null,
  requestReceivedAt: null,
  paymentLock: null,
  ...more,
});
const input = (over: Partial<BannerInput> = {}): BannerInput => ({
  detail: detail(),
  readError: null,
  watchOnly: false,
  lock: "unlocked",
  hasKeys: true,
  accepted: false,
  ...over,
});

describe("IOS-050 the payee and the tab's owner (D1, D5)", () => {
  it("names the payee as Contacts does, with its initials", () => {
    expect(payeeOf(summary())).toEqual({ contact: BOB, name: "Bob Builder", sub: "@bob", letters: "BB" });
    expect(payeeOf(summary({ displayName: null }))).toMatchObject({ name: "bob", sub: null, letters: "B" });
  });

  it("says why there is no one to pay without a main identity", () => {
    expect(noOwner({ kind: "join", reason: null })).toMatchObject({ title: "Join DashPay", next: "join" });
    expect(noOwner({ kind: "join", reason: "watchOnly" })).toMatchObject({ message: "A watch-only wallet cannot pay contacts.", next: null });
    expect(noOwner({ kind: "error" })).toMatchObject({ next: "retry" });
    expect(noOwner({ kind: "registering" })).toMatchObject({ next: null });
  });
});

describe("IOS-050 the picker (D4)", () => {
  it("lists My Contacts then Contact Requests in the engine's order, with why a row cannot be paid", () => {
    const groups = pickerGroups({
      updating: false,
      sections: [
        { section: "requests", contacts: [summary({ contact: "g", username: "grace", displayName: "Grace", relation: "pendingIncoming" })] },
        { section: "contacts", contacts: [summary(), summary({ contact: "d", username: "dave", displayName: null, channel: "broken" })] },
        { section: "pending", contacts: [summary({ contact: "f", relation: "pendingOutgoing" })] },
      ],
    });
    expect(groups.map((g) => [g.id, g.title, g.rows.map((r) => [r.name, r.reason])])).toEqual([
      [
        "contacts",
        "My Contacts",
        [
          ["Bob Builder", null],
          ["dave", "Payments cannot be sent right now"],
        ],
      ],
      ["requests", "Contact Requests", [["Grace", "Accept and pay"]]],
    ]);
    expect(pickerGroups({ updating: true, sections: [] })).toEqual([]);
    expect(pickerGroups(undefined)).toEqual([]);
  });
});

describe("IOS-050 the banner (D6)", () => {
  it("is nothing for a ready contact", () => {
    expect(payBanner(input())).toBeNull();
    expect(blocks(null)).toBe(false);
  });

  it("waits for the detail, and says when it cannot be read or is gone", () => {
    expect(payBanner(input({ detail: undefined }))).toEqual({ kind: "loading" });
    expect(payBanner(input({ detail: undefined, readError: "timeout" }))).toEqual({ kind: "readFailed", text: "Could not read this contact: timeout" });
    expect(payBanner(input({ detail: null }))).toMatchObject({ kind: "gone" });
    expect(payBanner(input({ detail: detail({ relation: "pendingOutgoing" }) }))).toMatchObject({ kind: "gone", text: "You can pay this user once you are contacts." });
    expect(payBanner(input({ detail: detail({ relation: "ignored" }) }))).toMatchObject({ kind: "gone" });
  });

  it("shows the payment lock with its txid before the channel, and only then mixing-only", () => {
    const locked = detail({ channel: "broken" }, { paymentLock: { txid: "d4".repeat(32), since: 5 } });
    expect(payBanner(input({ detail: locked, lock: "unlockedMixingOnly" }))).toEqual({ kind: "paymentLock", text: PAYMENT_LOCK_TEXT, txid: "d4".repeat(32) });
    expect(payBanner(input({ detail: detail({ channel: "broken" }), lock: "unlockedMixingOnly" }))).toMatchObject({ kind: "broken" });
    expect(payBanner(input({ lock: "unlockedMixingOnly" }))).toMatchObject({ kind: "mixingOnly" });
    expect(payBanner(input({ watchOnly: true, detail: locked }))).toMatchObject({ kind: "watchOnly" });
  });

  it("only notes a channel still being set up, or a request Accept and pay answers", () => {
    const setup = payBanner(input({ detail: detail({ channel: "setupPending" }) }));
    expect(setup).toMatchObject({ kind: "note" });
    expect(blocks(setup)).toBe(false);
    expect(payBanner(input({ detail: detail({ channel: "notEstablished" }) }))).toMatchObject({ kind: "note", text: expect.stringContaining("not set up yet") });
    const request = payBanner(input({ detail: detail({ relation: "pendingIncoming", channel: "notEstablished" }) }));
    expect(request).toMatchObject({ kind: "note", text: expect.stringContaining("Accept and pay") });
    expect(blocks(payBanner(input({ detail: detail({ relation: "pendingIncoming" }), hasKeys: false })))).toBe(true);
  });

  it("once this form accepted, pays without accepting again, before the re-read lands (D11)", () => {
    const request = detail({ relation: "pendingIncoming", channel: "notEstablished" });
    expect(acceptsToo(request, false)).toBe(true);
    expect(acceptsToo(request, true)).toBe(false);
    expect(acceptsToo(detail(), false)).toBe(false);
    expect(payBanner(input({ detail: request, accepted: true }))).toMatchObject({ kind: "note", text: expect.stringContaining("not set up yet") });
  });

  it("says what Check again found (D7)", () => {
    expect(resolveText("sent")).toEqual({ text: "The payment was sent. You can pay this contact again.", resolved: true });
    expect(resolveText("notSent")).toMatchObject({ resolved: true });
    expect(resolveText("unknown")).toEqual({ text: "Still not known. Check again once the wallet has synced.", resolved: false });
  });
});

describe("IOS-050 Accept and pay: one credential, one lease (D10, D11)", () => {
  const CRED = { kind: "passphrase", passphrase: "demo" } as const;
  const args = { walletId: WALLET, identity: ME, from: BOB, spendCap: 1_000_226, credential: CRED };
  const fake = (over: Partial<AcceptApi> = {}) => {
    const calls: string[] = [];
    const api: AcceptApi = {
      grantRequest: vi.fn(async () => (calls.push("grantRequest"), { maxDuffs: 0, maxCredits: "150000000" })),
      authorizeSet: vi.fn(async (purposes) => (calls.push("authorizeSet"), purposes.map((purpose: GrantPurposeSpec, i: number) => ({ id: `g${i}`, purpose, walletId: WALLET, expiresAt: 0, singleUse: true })))),
      beginFlow: vi.fn(async () => (calls.push("beginFlow"), "lease-" + "0".repeat(32))),
      dashpayAcceptRequest: vi.fn(async () => (calls.push("accept"), { kind: "established" as const, request: "1" })),
      endFlow: vi.fn(async () => (calls.push("endFlow"), null)),
      revokeGrant: vi.fn(async (id) => (calls.push(`revoke ${id}`), null)),
      ...over,
    };
    return { api, calls };
  };

  it("sizes the platformOp, checks the credential once for both grants, and accepts under the lease", async () => {
    const { api, calls } = fake();
    expect(await acceptUnderLease(api, args)).toBe("lease-" + "0".repeat(32));
    expect(calls).toEqual(["grantRequest", "authorizeSet", "beginFlow", "accept"]);
    expect(api.authorizeSet).toHaveBeenCalledWith(
      [
        { kind: "platformOp", maxDuffs: 0, maxCredits: "150000000" },
        { kind: "spend", maxDuffs: 1_000_226 },
      ],
      WALLET,
      CRED,
    );
    expect(api.beginFlow).toHaveBeenCalledWith(WALLET, "acceptAndPay", ["g0", "g1"]);
    expect(api.dashpayAcceptRequest).toHaveBeenCalledWith(WALLET, ME, BOB, "lease-" + "0".repeat(32));
  });

  it("takes a request someone else accepted first as accepted, and goes on under the lease", async () => {
    const { api, calls } = fake({ dashpayAcceptRequest: vi.fn(async () => Promise.reject(err("contact.already_contact"))) });
    expect(await acceptUnderLease(api, args)).toMatch(/^lease-/);
    expect(calls).not.toContain("endFlow");
  });

  it("a credential refusal is the authorize stage: nothing else ran", async () => {
    const { api, calls } = fake({ authorizeSet: vi.fn(async () => Promise.reject(err("vault.wrong_passphrase"))) });
    const e = await acceptUnderLease(api, args).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(AcceptFailed);
    expect(e).toMatchObject({ stage: "authorize", cause: { code: "vault.wrong_passphrase" } });
    expect(calls).toEqual(["grantRequest"]);
  });

  it("an accept refusal ends the lease; a lease refusal revokes the grants", async () => {
    const refused = fake({ dashpayAcceptRequest: vi.fn(async () => Promise.reject(err("platform.insufficient_credits"))) });
    await expect(acceptUnderLease(refused.api, args)).rejects.toMatchObject({ stage: "accept", cause: { code: "platform.insufficient_credits" } });
    expect(refused.calls.at(-1)).toBe("endFlow");
    const noLease = fake({ beginFlow: vi.fn(async () => Promise.reject(err("platform.grant_invalid"))) });
    await expect(acceptUnderLease(noLease.api, args)).rejects.toMatchObject({ stage: "accept" });
    expect(noLease.calls).toEqual(["grantRequest", "authorizeSet", "revoke g0", "revoke g1"]);
  });
});

describe("IOS-050 U-03's promise and refusals for a contact (D9, D13)", () => {
  const pay: ContactRecipient = { identity: ME, contact: BOB, amount: 1_000_000, subtractFee: false, note: "Lunch" };
  const estimate = { fee: 226, sizeBytes: 226, inputCount: 1, change: 10, totalSent: 1_000_000 };
  const confirmed = confirmedOf(pay, estimate, { kind: "auto" });
  const prepared = (outputs: PreparedTxSummary["outputs"], over: Partial<PreparedTxSummary> = {}): PreparedTxSummary => ({
    txid: "ab".repeat(32),
    fee: 226,
    feeRatePerKb: 1000,
    sizeBytes: 226,
    inputs: [],
    outputs,
    totalSent: 1_000_000,
    totalDebit: 1_000_226,
    externalSent: 1_000_000,
    ...over,
  });
  const change = { address: "yChange", amount: 10, isChange: true, label: null, isMine: true };
  const toContact = { address: "yReserved", amount: 1_000_000, isChange: false, label: null, isMine: false };

  it("tells a contact from an address", () => {
    expect(isContact(pay)).toBe(true);
    expect(subtractsFee({ ...pay, subtractFee: true })).toBe(true);
    expect(confirmed.total).toBe(1_000_226);
  });

  it("holds for one payment the wallet does not own, to whatever address the engine reserved", () => {
    expect(keepsPromise(confirmed, prepared([toContact, change]))).toBe(true);
    expect(keepsPromise(confirmed, prepared([{ ...toContact, address: "yAnother" }, change]))).toBe(true);
  });

  it("fails for a payment to the wallet itself, a second payee, or other amounts", () => {
    expect(keepsPromise(confirmed, prepared([{ ...toContact, isMine: true }, change]))).toBe(false);
    expect(keepsPromise(confirmed, prepared([{ ...toContact, amount: 500_000 }, { ...toContact, amount: 500_000 }, change]))).toBe(false);
    expect(keepsPromise(confirmed, prepared([toContact, change], { totalDebit: 1_000_227 }))).toBe(false);
    expect(keepsPromise(confirmed, prepared([toContact, change], { fee: 300 }))).toBe(false);
  });

  it("has no unsigned contact payment", () => {
    expect(unsignedKeepsPromise(confirmed, { outputs: [], fee: 226, unsignedInputs: 1 } as never)).toBe(false);
  });

  it("words every DP3-01 refusal", () => {
    const text = (e: AppError) => sendErrorText(e, "test")?.text;
    expect(text(err("contact.payment_locked", { kind: "txid", txid: "d4".repeat(32) }))).toMatch(/Another payment to this contact/);
    expect(text(err("contact.channel_broken"))).toBe("Payments to this contact cannot be sent right now.");
    expect(text(err("contact.ineligible", { kind: "ineligible", reason: "pendingIncoming" }))).toBe("Accept the contact request first, or use Accept and pay.");
    expect(text(err("contact.self"))).toBe("This is you.");
    expect(text(err("identity.not_found"))).toMatch(/DashPay identity is not available/);
    expect(text(err("platform.unavailable"))).toBe("Could not reach Dash Platform. Try again.");
    expect(text(err("platform.lease_revoked"))).toMatch(/Nothing was sent/);
    expect(text(err("platform.lease_expired"))).toBe("The authorization expired. Send again.");
    expect(text(err("platform.needs_grant"))).toBe("Unlock the wallet and try again.");
    expect(text(err("platform.grant_exceeded"))).toBe("The payment changed. Review it again.");
    expect(text(err("storage"))).toMatch(/could not be recorded, so it was not sent/);
  });

  it("a contact's first broadcast held back before its hand-off was not sent (DP3-01 D20, D23)", () => {
    const held = { draft: 1, prepared: 2, amount: 1_000_000, fee: 226 };
    expect(broadcastFailure(err("contact.payment_locked"), "test", held, true, "DASH", true)).toMatchObject({ kind: "failed" });
    expect(broadcastFailure(err("storage"), "test", held, true, "DASH", true)).toMatchObject({ kind: "failed" });
    // To an address, or on a retry, it stays unknown.
    expect(broadcastFailure(err("storage"), "test", held, true)).toMatchObject({ kind: "unknown" });
    expect(broadcastFailure(err("contact.payment_locked"), "test", held, false, "DASH", true)).toMatchObject({ kind: "unknown" });
  });
});
