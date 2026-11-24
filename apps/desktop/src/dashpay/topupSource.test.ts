// The top-up sheet's "Pay with" (C-routes D-C3, C3-8): the sources, each with its balance or the
// reason it cannot pay; a top-up from Platform's outcomes and refusals; its Activity row and the
// hold it keeps on the Platform addresses.

import { describe, expect, it } from "vitest";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import type { TopUpFromPlatformOutcome } from "../api/generated/TopUpFromPlatformOutcome";
import { platformActivityRefetchMs, PENDING_MOVE_REFETCH_MS } from "../api/queries";
import { isMayLandRow, platformRow, sourceRowTitle, SourceRowText } from "../history/sourceRows";
import { appError } from "../onboarding/testWorld";
import { hasPendingMove, hasUnansweredOp } from "../platform/move";
import { canPay, FROM_PLATFORM_COPY, fromPlatformErrorText, liveOutcome, maxMasked, offersMove, platformHold, platformOutcomeText, SOURCE_COPY, topUpSources, type SourceInputs } from "./topupSource";

const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

const addresses = (movable: string, duffs: number): PlatformAddressBalance => ({ credits: movable, movableCredits: movable, duffs, addresses: [], syncedAt: null, syncing: false, sendsActive: true, sendsOff: null });

const inputs = (patch: Partial<SourceInputs> = {}): SourceInputs => ({
  coreDuffs: 150_000_000,
  platform: addresses("40000000000", 40_000_000),
  hold: null,
  mainnet: false,
  shieldedDuffs: 2_000_000,
  amount,
  discreet: false,
  ...patch,
});

const row = (a: Partial<PlatformActivity>): PlatformActivity => ({
  id: "ti-1",
  kind: "toIdentity",
  state: "done",
  credits: "5001000000",
  address: null,
  to: "Identity1",
  expectedDuffs: null,
  feeCredits: "1000000",
  at: 1_700_000_000,
  mayLand: false,
  estimated: false,
  ...a,
});

const reasons = (i: SourceInputs) => Object.fromEntries(topUpSources(i).map((r) => [r.source, r.reason]));

describe("Pay with (C3-8)", () => {
  it("lists Dash wallet, Platform and Shielded with their balances; Shielded is not offered yet", () => {
    const rows = topUpSources(inputs());
    expect(rows.map((r) => [r.source, r.label, r.balance])).toEqual([
      ["core", SOURCE_COPY.core, "1.5 tDASH"],
      ["platform", SOURCE_COPY.platform, "0.4 tDASH"],
      ["shielded", SOURCE_COPY.shielded, "0.02 tDASH"],
    ]);
    expect(reasons(inputs())).toEqual({ core: null, platform: null, shielded: SOURCE_COPY.notOffered });
    expect(canPay(rows, "platform")).toBe(true);
    expect(canPay(rows, "shielded")).toBe(false);
  });

  it("discreet mode shows no balances", () => {
    expect(topUpSources(inputs({ discreet: true })).map((r) => r.balance)).toEqual([null, null, null]);
  });

  it("Platform is disabled with its reason: no address funds here, mainnet, nothing a top-up can take, or a hold", () => {
    expect(reasons(inputs({ platform: null })).platform).toBe(SOURCE_COPY.notOffered);
    expect(reasons(inputs({ mainnet: true })).platform).toBe(SOURCE_COPY.notOffered);
    expect(reasons(inputs({ platform: { ...addresses("40000000000", 40_000_000), sendsActive: false } })).platform).toBe(SOURCE_COPY.notOffered);
    expect(canPay(topUpSources(inputs({ platform: { ...addresses("40000000000", 40_000_000), sendsActive: false } })), "platform")).toBe(false);
    expect(reasons(inputs({ platform: undefined })).platform).toBe(SOURCE_COPY.loading);
    expect(reasons(inputs({ platform: addresses("0", 0) })).platform).toBe(SOURCE_COPY.nothing);
    expect(reasons(inputs({ hold: { state: "pending", topUp: true } })).platform).toBe(FROM_PLATFORM_COPY.pending.text);
    expect(reasons(inputs({ hold: { state: "held", topUp: true } })).platform).toBe(FROM_PLATFORM_COPY.held.text);
    expect(canPay(topUpSources(inputs({ mainnet: true })), "platform")).toBe(false);
  });

  it("addresses or Activity that could not be read say so, not that Platform is unavailable", () => {
    expect(reasons(inputs({ platform: "unread" })).platform).toBe(SOURCE_COPY.unread);
    expect(SOURCE_COPY.unread).not.toBe(SOURCE_COPY.notOffered);
    expect(topUpSources(inputs({ platform: "unread" }))[1].balance).toBeNull();
  });

  it("a send or move holding the addresses says so, not a top-up's copy", () => {
    expect(reasons(inputs({ hold: { state: "pending", topUp: false } })).platform).toBe(SOURCE_COPY.heldByOther);
    expect(reasons(inputs({ hold: { state: "held", topUp: false } })).platform).toBe(SOURCE_COPY.heldByOther);
  });

  it("a pending or held top-up, move or send holds the Platform addresses", () => {
    expect(platformHold([])).toBeNull();
    expect(platformHold([row({ state: "done" })])).toBeNull();
    expect(platformHold([row({ state: "pending", mayLand: true })])).toEqual({ state: "pending", topUp: true });
    expect(platformHold([row({ state: "failed", mayLand: true })])).toEqual({ state: "held", topUp: true });
    expect(platformHold([row({ state: "failed", mayLand: false })])).toBeNull();
    expect(platformHold([row({ kind: "moveToCore", state: "failed", mayLand: true }), row({ kind: "send", state: "pending", mayLand: true })])).toEqual({ state: "pending", topUp: false });
  });

  it("Move to Dash wallet is offered for a held top-up only, from Activity", () => {
    expect(offersMove(platformHold([row({ state: "failed", mayLand: true })]))).toBe(true);
    expect(offersMove(platformHold([row({ state: "pending", mayLand: true })]))).toBe(false);
    expect(offersMove(platformHold([row({ state: "failed", mayLand: true }), row({ id: "m-1", kind: "moveToCore", state: "failed", mayLand: true })]))).toBe(false);
    expect(offersMove(null)).toBe(false);
  });
});

describe("a top-up from Platform's outcome", () => {
  const balance = (credits: string) => `${credits} credits`;
  const outcome = (o: Partial<TopUpFromPlatformOutcome>): TopUpFromPlatformOutcome => ({ id: "ti-1", credits: "5000000000", feeCredits: "1000000", state: "done", mayLand: false, balance: "9000000000", ...o });

  it("done: the credits added and the balance", () => {
    const o = platformOutcomeText(outcome({}), balance);
    expect(o.move).toBe(false);
    expect(o.text).toContain("9000000000 credits");
  });

  it("discreet mode masks a Max top-up's credits on the outcome", () => {
    const masked = maxMasked(true, true, "***");
    expect(masked).toBe("***");
    expect(maxMasked(true, false, "***")).toBeNull();
    expect(maxMasked(false, true, "***")).toBeNull();
    const o = platformOutcomeText(outcome({ balance: null }), balance, masked);
    expect(o.text).toContain("***");
    expect(o.text.replace(/\D/g, "")).toBe("");
  });

  it("pending: may still go through, no action", () => {
    expect(platformOutcomeText(outcome({ state: "pending", mayLand: true, balance: null }), balance)).toEqual({ ...FROM_PLATFORM_COPY.pending, move: false });
  });

  it("'Nothing left' only when it can never execute; anything else not done is held, with Move to Dash wallet", () => {
    const never = platformOutcomeText(outcome({ state: "failed", mayLand: false, balance: null }), balance);
    expect(never).toEqual({ title: FROM_PLATFORM_COPY.failed.title, text: FROM_PLATFORM_COPY.failed.text, move: false });
    const held = platformOutcomeText(outcome({ state: "failed", mayLand: true, balance: null }), balance);
    expect(held).toEqual({ title: FROM_PLATFORM_COPY.held.title, text: FROM_PLATFORM_COPY.held.text, move: true });
    expect(platformOutcomeText(outcome({ state: "replaced", mayLand: false, balance: null }), balance).move).toBe(true);
  });

  it("follows its Activity row: a pending top-up later held offers the move; one that can never land says nothing left", () => {
    const sent = outcome({ state: "pending", mayLand: true, balance: null });
    expect(liveOutcome(sent, undefined)).toBe(sent);
    expect(liveOutcome(sent, [row({ id: "other", state: "failed", mayLand: true })])).toEqual(sent);
    const held = liveOutcome(sent, [row({ state: "failed", mayLand: true })]);
    expect(platformOutcomeText(held, balance).move).toBe(true);
    const never = liveOutcome(sent, [row({ state: "failed", mayLand: false })]);
    expect(platformOutcomeText(never, balance)).toEqual({ ...FROM_PLATFORM_COPY.failed, move: false });
  });

  it("refusals: a hold, the range, the balance and mainnet", () => {
    expect(fromPlatformErrorText(appError("credits.move_pending", { kind: "artifact", artifact: "ti-1" }), amount)).toBe(FROM_PLATFORM_COPY.pending.text);
    expect(fromPlatformErrorText(appError("credits.send_out_of_range", { kind: "creditMinimum", min: "200000000" }), amount)).toBe(FROM_PLATFORM_COPY.error.below("0.002 tDASH"));
    expect(fromPlatformErrorText(appError("platform.insufficient_credits", { kind: "credits", needed: "1", available: "0" }), amount)).toBe(FROM_PLATFORM_COPY.error.short);
    expect(fromPlatformErrorText(appError("platform.feature_off", { kind: "feature", feature: "top_up_from_platform" }), amount)).toBe(SOURCE_COPY.notOffered);
  });
});

describe("a top-up from Platform in Activity (C3-5)", () => {
  it("is a debit row with its own title, unconfirmed while it may still go through", () => {
    const done = platformRow(row({}));
    expect([sourceRowTitle(done), done.amount, done.status.kind]).toEqual([SourceRowText.toIdentity, -5_001_000, "confirmed"]);
    const held = platformRow(row({ state: "failed", mayLand: true }));
    expect([isMayLandRow(held), held.status.kind]).toEqual([true, "unconfirmed"]);
    expect(platformRow(row({ state: "failed", mayLand: false })).status.kind).toBe("notAccepted");
  });

  it("holds the next move and keeps Activity polling until it settles", () => {
    expect(hasPendingMove([row({ state: "pending", mayLand: true })])).toBe(true);
    expect(hasUnansweredOp([row({ state: "pending", mayLand: true })])).toBe(true);
    expect(hasUnansweredOp([row({ state: "failed", mayLand: true })])).toBe(false);
    expect(platformActivityRefetchMs([row({ state: "failed", mayLand: true })])).toBe(PENDING_MOVE_REFETCH_MS);
    expect(platformActivityRefetchMs([row({ state: "done" })])).toBe(false);
  });
});
