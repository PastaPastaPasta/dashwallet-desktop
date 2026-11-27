// The fixture's payment to a contact from a balance (C-funding D-C5), through the generated
// commands as the Pay tab calls them: the quote's refusals in the engine's order, a payout under
// the grant its quote sizes, and each outcome: sent, unknown (the contact locked until its
// evidence), not sent.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";

const ME = IDENTITY.alice;
const BOB = IDENTITY.bob;
const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
/** 0.1 Dash in credits. */
const CREDITS = "10000000000";

let fixture: FixtureHandle;
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "dashpay-registered" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const refusal = (p: Promise<unknown>) => p.then(() => null, (e: cmd.AppError) => e.code);

async function pay(credits = CREDITS) {
  const q = await cmd.contactPayQuote(W, ME, BOB, credits, "platform");
  const [grant] = await cmd.authorizeSet([{ kind: "platformOp", maxDuffs: q.grant.maxDuffs, maxCredits: q.grant.maxCredits }], W, PASS);
  return cmd.contactPay(W, ME, BOB, { credits, source: "platform", note: "lunch", confirmUnrecorded: false }, grant.id);
}

describe("fixture contact payment from a balance", () => {
  it("quotes the amount, about what arrives, the fee, and a grant with three fee estimates", async () => {
    const q = await cmd.contactPayQuote(W, ME, BOB, CREDITS, "platform");
    expect(q).toMatchObject({ source: "platform", credits: CREDITS, feeCredits: "2000000" });
    expect(q.expectedDuffs).toBeLessThan(10_000_000);
    expect(q.grant.maxCredits).toBe(String(BigInt(CREDITS) + 3n * 2_000_000n));
  });

  it("refuses the Dash wallet, Shielded until its slice, a zero amount and below the minimum", async () => {
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, CREDITS, "core"))).toBe("invalid_argument");
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, "0", "platform"))).toBe("invalid_argument");
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, CREDITS, "shielded"))).toBe("platform.feature_off");
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, "1000", "platform"))).toBe("credits.withdrawal_out_of_range");
  });

  it("refuses Shielded as this build's engine does where spends from it are off: after the payee's checks", async () => {
    clearMocks();
    fixture = installFixture({ scenario: "dashpay-registered-spends-off" });
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, CREDITS, "shielded"))).toBe("platform.unavailable");
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, "0", "shielded"))).toBe("invalid_argument");
    expect(await refusal(cmd.contactPayQuote(W, ME, IDENTITY.alice, CREDITS, "shielded"))).not.toBe("platform.unavailable");
  });

  it("pays: sent, the balance debited and a contact payment row naming the contact", async () => {
    const before = await cmd.platformAddresses(W);
    const out = await pay();
    expect(out.state).toBe("sent");
    const after = await cmd.platformAddresses(W);
    expect(BigInt(before!.credits) - BigInt(after!.credits)).toBe(BigInt(CREDITS) + 2_000_000n);
    const [row] = (await cmd.platformActivity(W))!;
    expect(row).toMatchObject({ id: out.reference, kind: "contactPayment", state: "done", contact: { identity: BOB } });
    expect((await cmd.dashpayContact(W, ME, BOB))?.paymentLock).toBeNull();
  });

  it("an unknown outcome locks the contact and holds the balance until its evidence", async () => {
    fixture.world.pending.platform!.nextPayout = "pending";
    const out = await pay();
    expect(out.state).toBe("pending");
    expect((await cmd.dashpayContact(W, ME, BOB))?.paymentLock).toMatchObject({ source: "platform", reference: out.reference });
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, CREDITS, "platform"))).toBe("contact.payment_locked");
    expect(await cmd.resolveContactPaymentLock(W, ME, BOB)).toBe("unknown");
  });

  it("a payout not sent leaves the balance and the contact as they were", async () => {
    fixture.world.pending.platform!.nextPayout = "notSent";
    const before = await cmd.platformAddresses(W);
    expect((await pay()).state).toBe("notSent");
    expect((await cmd.platformAddresses(W))!.credits).toBe(before!.credits);
    expect((await cmd.dashpayContact(W, ME, BOB))?.paymentLock).toBeNull();
  });

  it("is off on mainnet before anything else, as the engine's refuse_on_mainnet", async () => {
    fixture.world.launch.network = { kind: "mainnet", dirName: "mainnet", unitName: "DASH" };
    expect(await refusal(cmd.contactPayQuote(W, ME, BOB, CREDITS, "platform"))).toBe("platform.feature_off");
    expect(await refusal(cmd.contactPay(W, ME, BOB, { credits: CREDITS, source: "platform", note: null, confirmUnrecorded: false }, ""))).toBe("platform.feature_off");
  });

  it("needs the amount and three fee estimates on the balance, as the engine's reserve", async () => {
    const p = fixture.world.pending.platform!;
    p.addresses = [{ address: p.addresses[0].address, credits: String(BigInt(CREDITS) + 2n * 2_000_000n) }];
    const short = await cmd.contactPayQuote(W, ME, BOB, CREDITS, "platform").then(
      () => null,
      (e: cmd.AppError) => e,
    );
    expect(short).toMatchObject({ code: "platform.insufficient_credits", data: { needed: String(BigInt(CREDITS) + 3n * 2_000_000n) } });
    p.addresses = [{ address: p.addresses[0].address, credits: String(BigInt(CREDITS) + 3n * 2_000_000n) }];
    expect((await cmd.contactPayQuote(W, ME, BOB, CREDITS, "platform")).credits).toBe(CREDITS);
  });

  it("a lost reply: the payout was recorded and the call still fails as internal", async () => {
    fixture.world.pending.platform!.loseReply = true;
    expect(await refusal(pay())).toBe("internal");
    const [row] = (await cmd.platformActivity(W))!;
    expect(row).toMatchObject({ kind: "contactPayment", state: "done", contact: { identity: BOB } });
  });

  it("refuses a grant smaller than the quote's", async () => {
    const [grant] = await cmd.authorizeSet([{ kind: "platformOp", maxDuffs: 0, maxCredits: CREDITS }], W, PASS);
    expect(await refusal(cmd.contactPay(W, ME, BOB, { credits: CREDITS, source: "platform", note: null, confirmUnrecorded: false }, grant.id))).toBe("platform.grant_exceeded");
  });
});
