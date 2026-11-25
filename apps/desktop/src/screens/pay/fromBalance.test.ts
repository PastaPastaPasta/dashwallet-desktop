import { describe, expect, it, vi } from "vitest";
import type { ContactPayQuoteInfo } from "../../api/generated/ContactPayQuoteInfo";
import { appError, WALLET_A } from "../../onboarding/testWorld";
import { fakeVault } from "../../tools/testApi";
import type { PlatformActivity } from "../../api/generated/PlatformActivity";
import { chosenReason, ContactPayFlow, fromErrorText, fromRows, liveSources, outcomeText, totalText, type FromInputs } from "./fromBalance";

const amount = (d: number) => `${d / 1e8} tDASH`;
const credits = (c: string) => amount(Number(BigInt(c) / 1000n));
const base: FromInputs = { coreDuffs: 5e8, shielded: 2e8, platform: 3e8, amount, discreet: false, acceptsToo: false };

describe("the From step (C-funding P16)", () => {
  it("lists Dash wallet, Shielded and Platform, each with its balance", () => {
    expect(fromRows(base).map((r) => [r.source, r.label, r.balance])).toEqual([
      ["core", "Dash wallet", "5 tDASH"],
      ["shielded", "Shielded balance", "2 tDASH"],
      ["platform", "Platform balance", "3 tDASH"],
    ]);
  });

  it("lists a source that cannot pay with its reason, never hides it", () => {
    const rows = fromRows({ ...base, platform: "absent" });
    expect(rows.find((r) => r.source === "shielded")?.reason).toBe("Not available on this network yet.");
    expect(rows.find((r) => r.source === "platform")?.reason).toBe("No balance to pay with.");
    expect(rows.find((r) => r.source === "core")?.reason).toBeNull();
    expect(rows).toHaveLength(3);
  });

  it("keeps the balances off on mainnet", () => {
    const rows = fromRows({ ...base, live: liveSources("mainnet") });
    expect(rows.map((r) => r.reason)).toEqual([null, "Not available on this network yet.", "Not available on this network yet."]);
    expect(chosenReason(rows, "platform")).toBe("Not available on this network yet.");
    expect(fromRows({ ...base, live: liveSources("testnet") }).find((r) => r.source === "platform")?.reason).toBeNull();
  });

  it("leaves Accept and pay to the Dash wallet", () => {
    expect(fromRows({ ...base, acceptsToo: true }).find((r) => r.source === "platform")?.reason).toBe("Accept and pay can only use your Dash wallet balance.");
  });

  it("says an empty balance has nothing to pay with, in MG's words", () => {
    expect(fromRows({ ...base, platform: 0 }).find((r) => r.source === "platform")?.reason).toBe("No balance to pay with.");
    expect(fromRows({ ...base, platform: 0, acceptsToo: true }).find((r) => r.source === "platform")?.reason).toBe("Accept and pay can only use your Dash wallet balance.");
  });

  it("shows no balance in discreet mode or while unread", () => {
    expect(fromRows({ ...base, discreet: true }).map((r) => r.balance)).toEqual([null, null, null]);
    expect(fromRows({ ...base, platform: "loading" }).find((r) => r.source === "platform")).toMatchObject({ balance: null, reason: null });
  });

  it("never moves a choice to the Dash wallet: a source that can no longer pay gives its reason", () => {
    expect(chosenReason(fromRows(base), "platform")).toBeNull();
    // A balance read late, or refreshed to nothing, after Platform was chosen.
    expect(chosenReason(fromRows({ ...base, platform: "loading" }), "platform")).toBeNull();
    expect(chosenReason(fromRows({ ...base, platform: 0 }), "platform")).toBe("No balance to pay with.");
    expect(chosenReason(fromRows(base), "shielded")).toBe("Not available on this network yet.");
    expect(chosenReason(fromRows({ ...base, platform: 0 }), "core")).toBeNull();
  });
});

describe("a payment from a balance's copy (D-C5 Copy)", () => {
  it("says the outcome", () => {
    expect(outcomeText("sent", "@bob").text).toBe("Payment sent. It arrives in @bob’s wallet once the Dash network processes it. This can take a while.");
    expect(outcomeText("pending", "@bob")).toEqual({ title: "Payment status unknown", text: "We could not confirm this payment. It may still go through. Do not pay again until this is settled." });
    expect(outcomeText("notSent", "@bob").title).toBe("Payment not sent");
  });

  it("names the minimum below it, and a pending Platform transfer", () => {
    const range = { code: "credits.withdrawal_out_of_range", message: "", data: { kind: "creditRange", min: "1190000", max: "50000000000000" } };
    expect(fromErrorText(range, "1000", credits)).toBe("The smallest payment from here is 0.0000119 tDASH. Pay a smaller amount from your Dash wallet.");
    expect(fromErrorText({ code: "credits.move_pending", message: "", data: null }, "1", credits)).toBe("Another Platform transfer is still being confirmed. Try again when it is done.");
    expect(fromErrorText({ code: "platform.feature_off", message: "", data: null }, "1", credits)).toBe("Not available on this network yet.");
  });

  it("names no amount a masked formatter hides (discreet mode)", () => {
    const short = { code: "platform.insufficient_credits", message: "", data: { kind: "credits", needed: "10002000000", available: "4000000000" } };
    expect(fromErrorText(short, "10000000000", credits)).toContain("0.04 tDASH");
    const masked = fromErrorText(short, "10000000000", () => "••••");
    expect(masked).not.toMatch(/\d\.\d/);
    expect(masked).toContain("••••");
  });
});

const QUOTE: ContactPayQuoteInfo = { source: "platform", credits: "10000000000", expectedDuffs: 9_999_774, feeCredits: "2000000", grant: { maxDuffs: 0, maxCredits: "10006000000" } };

function payFlow(contactPay: () => Promise<never>, lock = { encrypted: false }, platformActivity: () => Promise<PlatformActivity[] | null> = () => Promise.resolve([])) {
  const vault = fakeVault(lock);
  return new ContactPayFlow({ ...vault.api, contactPay: vi.fn(contactPay), platformActivity: vi.fn(platformActivity) }, WALLET_A, "me", "bob");
}

const payout = (over: Partial<PlatformActivity>): PlatformActivity =>
  ({ id: "move-9", kind: "contactPayment", state: "done", credits: "10002000000", address: null, to: null, expectedDuffs: 9_999_774, feeCredits: "2000000", at: 0, mayLand: false, estimated: false, contact: { identity: "bob", username: null, displayName: null, avatarUrl: null }, ...over }) as PlatformActivity;

describe("a payment from a balance's flow", () => {
  it("a failure after the call was sent may have paid: unknown, never 'nothing was paid'", async () => {
    const lost = payFlow(() => Promise.reject(new Error("the connection to the engine closed")));
    await lost.pay(QUOTE, null);
    expect(lost.step).toBe("lost");
    const internal = payFlow(() => Promise.reject(appError("internal")));
    await internal.pay(QUOTE, null);
    expect(internal.step).toBe("lost");
  });

  it("an outcome the engine could not confirm may already be out: unknown and held, never 'nothing was paid'", async () => {
    for (const code of ["platform.broadcast_unknown", "platform.will_be_sent"] as const) {
      const flow = payFlow(() => Promise.reject(appError(code)));
      await flow.pay(QUOTE, null);
      expect([code, flow.step, flow.unresolved !== null]).toEqual([code, "lost", true]);
    }
    const refused = payFlow(() => Promise.reject(appError("platform.insufficient_credits")));
    await refused.pay(QUOTE, null);
    expect([refused.step, refused.unresolved]).toEqual(["refused", null]);
  });

  it("a lost reply keeps the contact held after the sheet closes, until a later read settles it", async () => {
    const flow = payFlow(() => Promise.reject(appError("internal")));
    await flow.pay(QUOTE, null);
    flow.reset();
    expect([flow.step, flow.unresolved !== null]).toEqual(["confirm", true]);
    const { lostAt, since } = flow.unresolved!;
    const row = (over: Partial<PlatformActivity>): PlatformActivity =>
      ({ id: "move-9", kind: "contactPayment", state: "done", credits: "10002000000", address: null, to: null, expectedDuffs: 9_999_774, feeCredits: "2000000", at: since + 1, mayLand: false, estimated: false, contact: { identity: "bob", username: null, displayName: null, avatarUrl: null }, ...over }) as PlatformActivity;
    // A read from before the loss, a failed read, or a payout that may still land: still held.
    expect(flow.reconcile([row({})], lostAt - 1)).toBeNull();
    expect(flow.reconcile(undefined, lostAt + 1)).toBeNull();
    expect(flow.reconcile([row({ state: "pending", mayLand: true })], lostAt + 1)).toBeNull();
    expect(flow.reconcile([row({ state: "failed", mayLand: true })], lostAt + 1)).toBeNull();
    expect(flow.unresolved).not.toBeNull();
    // Recorded as sent: it was. An older payout, or another contact's, says nothing of this one.
    expect(flow.reconcile([row({})], lostAt + 1)).toBe("sent");
    expect(flow.unresolved).toBeNull();
    const again = payFlow(() => Promise.reject(new Error("closed")));
    await again.pay(QUOTE, null);
    const at = again.unresolved!.lostAt + 1;
    const older = row({ at: again.unresolved!.since - 60 });
    const other = row({ contact: { identity: "carol", username: null, displayName: null, avatarUrl: null } });
    expect(again.reconcile([older, other, row({ state: "failed", mayLand: false })], at)).toBe("notSent");
  });

  it("payout rows the read before the attempt listed say nothing of it; a new one does, named or not", async () => {
    const flow = payFlow(() => Promise.reject(appError("internal")));
    await flow.pay(QUOTE, null, [payout({ id: "move-1" })]);
    const at = flow.unresolved!.lostAt;
    expect(flow.reconcile([payout({ id: "move-1" })], at)).toBe("notSent");
    const again = payFlow(() => Promise.reject(appError("internal")));
    await again.pay(QUOTE, null, [payout({ id: "move-1" })]);
    expect(again.reconcile([payout({ id: "move-1" }), payout({ id: "move-2", state: "pending", mayLand: true, contact: undefined })], at)).toBeNull();
    expect(again.reconcile([payout({ id: "move-1" }), payout({ id: "move-2", contact: undefined })], again.unresolved!.lostAt)).toBe("sent");
  });

  it("Check again reads after the loss; a failed read settles nothing", async () => {
    let rows: Promise<PlatformActivity[] | null> = Promise.reject(new Error("offline"));
    const flow = payFlow(() => Promise.reject(appError("internal")), undefined, () => rows);
    await flow.pay(QUOTE, null, []);
    expect(await flow.recheck()).toBeNull();
    expect(flow.unresolved).not.toBeNull();
    rows = Promise.resolve([payout({ state: "pending", mayLand: true })]);
    expect(await flow.recheck()).toBeNull();
    rows = Promise.resolve([payout({})]);
    expect(await flow.recheck()).toBe("sent");
    expect([flow.unresolved, flow.checking]).toEqual([null, false]);
  });

  it("no payment starts while a lost reply is unsettled", async () => {
    const contactPay = vi.fn(() => Promise.reject(appError("internal")));
    const flow = new ContactPayFlow({ ...fakeVault({ encrypted: false }).api, contactPay, platformActivity: vi.fn(() => Promise.resolve([])) }, WALLET_A, "me", "bob");
    await flow.pay(QUOTE, null);
    flow.reset();
    await flow.pay(QUOTE, null);
    expect(contactPay).toHaveBeenCalledTimes(1);
  });

  it("a payout the engine never recorded was not sent", async () => {
    const flow = payFlow(() => Promise.reject(appError("internal")));
    await flow.pay(QUOTE, null);
    expect(flow.reconcile([], flow.unresolved!.lostAt + 1)).toBe("notSent");
  });

  it("a coded refusal is one: nothing was paid", async () => {
    const refused = payFlow(() => Promise.reject(appError("credits.move_pending")));
    await refused.pay(QUOTE, null);
    expect([refused.step, refused.unresolved]).toEqual(["refused", null]);
  });

  it("a prompt cancelled (the sheet went with a lock) leaves the flow idle, nothing sent", async () => {
    const send = vi.fn(() => Promise.reject(new Error("never called")));
    const flow = payFlow(send, { encrypted: true });
    const paying = flow.pay(QUOTE, null);
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await paying;
    expect([flow.step, flow.running]).toEqual(["confirm", false]);
    expect(send).not.toHaveBeenCalled();
  });
});

describe("the confirm's total", () => {
  it("is the amount and the quoted Platform fee, each named", () => {
    const quote = { ...QUOTE, credits: "10000000000", feeCredits: "2000000" };
    const shown = (c: string) => `${c} cr`;
    expect(totalText(quote, "Bob", shown)).toBe(`About ${String(10_000_000_000n + 2_000_000n)} cr leaves your Platform balance: 10000000000 cr to Bob and about 2000000 cr in Platform fees.`);
  });
});
