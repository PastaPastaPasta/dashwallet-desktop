// The fixture's shared masternodes (shared.ts) through the generated commands: a coordinator's
// session from start to locked terms, a participant's join, and Discard only while `discardable`.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { StartSharedRequest } from "../api/generated/StartSharedRequest";
import { installFixture } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { abandonFee, SH_TEXT } from "./shared";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const COIN = 100_000_000;

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const request = (amounts: number[] = [400, 300, 300]): StartSharedRequest => ({
  shares: amounts.map((a, i) => ({ label: ["Alice", "Bob", "Carol"][i] ?? `P${i}`, amount: a * COIN })),
  terms: { service: "203.0.113.30:19999", operator: { kind: "next_unused" }, operatorReward: 500, voting: { kind: "next_unused" }, earlyPeriodBlocks: 105_120, earlyPenalty: 20 * COIN },
  fee: 100_000,
  reward: { kind: "new_address" },
  nodeRunBy: null,
});

const grant = async (maxDuffs = 0) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs }, W, PASS)).id;

describe("fixture shared masternodes", () => {
  it("runs a coordinator's session from start to locked terms", async () => {
    installFixture({ scenario: "sh-capability-active" });
    await expect(cmd.mnSharedStart(W, request([500, 400]))).rejects.toMatchObject({ code: "invalid_argument" });
    const started = await cmd.mnSharedStart(W, request());
    expect(started.session).toMatchObject({ role: "coordinator", stage: { kind: "draft" }, discardable: true });
    expect(started.message.fileName).toMatch(/^shared-mn-session-.+\.json$/);
    const id = started.session.id;
    await cmd.mnSharedImport(W, SH_TEXT.bobReply, id);
    const ready = await cmd.mnSharedImport(W, SH_TEXT.carolReply, id);
    expect(ready.kind === "session" && ready.session.next).toBe("lock");
    const s = await cmd.mnSharedSession(W, id);
    await expect(cmd.mnSharedLock(W, id, s.revision - 1, await grant())).rejects.toMatchObject({ code: "masternodes.shared_message" });
    const locked = await cmd.mnSharedLock(W, id, s.revision, await grant());
    expect(locked.session.stage).toEqual({ kind: "signing" });
    expect((await cmd.mnSharedUnlock(W, id)).session.stage).toEqual({ kind: "draft" });
  });

  it("joins an invitation and discards it", async () => {
    installFixture({ scenario: "sh-capability-active" });
    const imported = await cmd.mnSharedImport(W, SH_TEXT.invitation, null);
    if (imported.kind !== "session") throw new Error("not a session");
    expect(imported.session.next).toBe("join");
    const joined = await cmd.mnSharedJoin(W, imported.session.id, { shareIndex: null, reward: { kind: "refund" } });
    expect(joined.session.rows.some((r) => r.mine)).toBe(true);
    await cmd.mnSharedDiscard(W, imported.session.id);
    expect(await cmd.mnSharedSessions(W)).toEqual([]);
  });

  it("lists unreadable sessions; Discard frees one that may not be out and refuses the other", async () => {
    installFixture({ scenario: "sh-unreadable" });
    const listed = await cmd.mnSharedUnreadable(W);
    const free = listed.find((s) => s.discardable)!;
    const out = listed.find((s) => !s.discardable)!;
    expect(listed.map((s) => s.discardable).sort()).toEqual([false, false, true]);
    await expect(cmd.mnSharedDiscard(W, out.id)).rejects.toMatchObject({ code: "masternodes.shared_stage" });
    await cmd.mnSharedDiscard(W, free.id);
    const left = (await cmd.mnSharedUnreadable(W)).map((s) => s.id);
    expect(left).toContain(out.id);
    expect(left).not.toContain(free.id);
    expect(await cmd.mnSharedSessions(W)).toHaveLength(1);
  });

  it("abandons a session that may be out under a grant of exactly its listed charge, then offers it no more", async () => {
    installFixture({ scenario: "sh-close-out" });
    const [s] = await cmd.mnSharedSessions(W);
    expect([s.discardable, s.abandonable]).toEqual([false, true]);
    const charge = s.abandonCharge!;
    expect(charge, "every reserved coin spent").toBe(abandonFee(s.reserved.length));
    const cap = async (max: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
    await expect(cmd.mnSharedAbandon(W, s.id, await cap(charge - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const after = await cmd.mnSharedAbandon(W, s.id, await cap(charge));
    expect(after).toMatchObject({ id: s.id, stage: { kind: "funding_signed" }, discardable: false, abandonable: false, abandonCharge: null });
    await expect(cmd.mnSharedAbandon(W, s.id, await cap(charge))).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "abandoning" } });
  });

  it("once abandoned, hands the message on no more and reads the spend back as recorded", async () => {
    installFixture({ scenario: "sh-close-out" });
    const [s] = await cmd.mnSharedSessions(W);
    expect(s.next).toBe("copy");
    await cmd.mnSharedMessage(W, s.id);
    const cap = async (max: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
    const after = await cmd.mnSharedAbandon(W, s.id, await cap(s.abandonCharge!));
    expect(after).toMatchObject({ next: "nothing", abandonSpend: { state: "in_history", code: null }, resendAbandonCharge: s.abandonCharge, activityAbandonTxids: ["a8".repeat(32)] });
    await expect(cmd.mnSharedMessage(W, s.id)).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "abandoning" } });
  });

  it("an uncertain spend back offers Send again under exactly its charge, and no new spend", async () => {
    installFixture({ scenario: "sh-close-out-abandon-uncertain" });
    const [s] = await cmd.mnSharedSessions(W);
    const fee = s.abandonCharge!;
    const cap = async (max: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
    const after = await cmd.mnSharedAbandon(W, s.id, await cap(fee));
    expect(after).toMatchObject({ abandonSpend: { state: "uncertain" }, abandonCharge: null, resendAbandonCharge: fee, activityAbandonTxids: [] });
    await expect(cmd.mnSharedAbandon(W, s.id, await cap(fee)), "no new spend while it may be out").rejects.toMatchObject({ data: { stage: "abandoning" } });
    await expect(cmd.mnSharedResendAbandon(W, s.id, await cap(fee - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    expect(await cmd.mnSharedResendAbandon(W, s.id, await cap(fee))).toMatchObject({ abandonSpend: { state: "in_history" }, activityAbandonTxids: ["a8".repeat(32)] });
  });

  it("a refused spend back offers a new spend of the same coins, and no Send again", async () => {
    installFixture({ scenario: "sh-close-out-abandon-refused" });
    const [s] = await cmd.mnSharedSessions(W);
    const fee = s.abandonCharge!;
    const cap = async (max: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
    const after = await cmd.mnSharedAbandon(W, s.id, await cap(fee));
    expect(after).toMatchObject({ abandonSpend: { state: "refused", code: "send.broadcast_rejected" }, abandonCharge: fee, resendAbandonCharge: null });
    await expect(cmd.mnSharedResendAbandon(W, s.id, await cap(fee))).rejects.toMatchObject({ data: { stage: "abandon_refused" } });
    expect(await cmd.mnSharedAbandon(W, s.id, await cap(fee))).toMatchObject({ abandonSpend: { state: "in_history" } });
  });

  it("refuses Abandon of a session not out, or settled, as the engine does", async () => {
    installFixture({ scenario: "sh-participant-joined" });
    const [s] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedAbandon(W, s.id, await grant())).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "draft" } });
    installFixture({ scenario: "sh-participant-confirmed" });
    const [c] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedAbandon(W, c.id, await grant())).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "confirmed" } });
  });

  it("abandons an unreadable session that may be out, and nothing else", async () => {
    installFixture({ scenario: "sh-unreadable" });
    const [free, out] = await cmd.mnSharedUnreadable(W);
    expect([free.abandonable, out.abandonable]).toEqual([false, true]);
    expect([free.abandonCharge, out.abandonCharge]).toEqual([null, abandonFee(out.reserved.length)]);
    const cap = async (max = out.abandonCharge!) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
    await expect(cmd.mnSharedAbandonUnreadable(W, out.id, await cap(out.abandonCharge! - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    await expect(cmd.mnSharedAbandonUnreadable(W, free.id, await cap())).rejects.toMatchObject({ code: "masternodes.shared_stage" });
    const [readable] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedAbandonUnreadable(W, readable.id, await cap())).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnSharedAbandon(W, out.id, await cap())).rejects.toMatchObject({ code: "internal" });
    const after = await cmd.mnSharedAbandonUnreadable(W, out.id, await cap());
    expect(after).toMatchObject({ id: out.id, discardable: false, abandonable: false });
    expect((await cmd.mnSharedUnreadable(W)).find((x) => x.id === out.id)?.abandonable).toBe(false);
  });

  it("abandons on mainnet as on every network", async () => {
    for (const network of ["mainnet", "testnet", "regtest"] as const) {
      installFixture({ scenario: "sh-close-out", network });
      const cap = async (max: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: max }, W, PASS)).id;
      const [s] = await cmd.mnSharedSessions(W);
      expect(await cmd.mnSharedAbandon(W, s.id, await cap(s.abandonCharge!))).toMatchObject({ id: s.id, abandonable: false });
      installFixture({ scenario: "sh-unreadable", network });
      const [, out] = await cmd.mnSharedUnreadable(W);
      expect(await cmd.mnSharedAbandonUnreadable(W, out.id, await cap(out.abandonCharge!))).toMatchObject({ id: out.id, abandonable: false });
    }
  });

  it("refuses Discard once the session may be out", async () => {
    installFixture({ scenario: "sh-close-out" });
    const [s] = await cmd.mnSharedSessions(W);
    expect(s.discardable).toBe(false);
    await expect(cmd.mnSharedDiscard(W, s.id)).rejects.toMatchObject({ code: "masternodes.shared_stage" });
  });

  it("refuses a message for another network or session without its text", async () => {
    installFixture({ scenario: "sh-coordinator-draft" });
    await expect(cmd.mnSharedImport(W, SH_TEXT.wrongNetwork, null)).rejects.toMatchObject({ code: "masternodes.shared_message", data: { kind: "sharedMessage", reason: "network" } });
    await expect(cmd.mnSharedImport(W, SH_TEXT.otherSession, null)).rejects.toMatchObject({ data: { reason: "other_session" } });
  });

  it("signs a participant's funding under a grant of its debit, and refuses one below it", async () => {
    installFixture({ scenario: "sh-participant-combined" });
    const [s] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedSign(W, s.id, await grant(s.debit! - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const signed = await cmd.mnSharedSign(W, s.id, await grant(s.debit!));
    expect(signed.session).toMatchObject({ stage: { kind: "funding_signed" }, next: "copy", discardable: false });
    await expect(cmd.mnSharedSign(W, s.id, await grant(s.debit!))).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "funding_signed" } });
    await expect(cmd.mnSharedSend(W, s.id, await grant(s.debit!))).rejects.toMatchObject({ data: { stage: "participant" } });
  });

  it("sends the registration once every other input is signed, then sends it again", async () => {
    installFixture({ scenario: "sh-coordinator-combined" });
    const [waiting] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedSend(W, waiting.id, await grant(waiting.debit!))).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "unsigned" } });
    installFixture({ scenario: "sh-coordinator-send" });
    const [s] = await cmd.mnSharedSessions(W);
    await expect(cmd.mnSharedSign(W, s.id, await grant(s.debit!))).rejects.toMatchObject({ data: { stage: "coordinator" } });
    await expect(cmd.mnSharedSend(W, s.id, await grant(s.debit! - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const sent = await cmd.mnSharedSend(W, s.id, await grant(s.debit!));
    expect(sent.session).toMatchObject({ stage: { kind: "sent" }, next: "wait", discardable: false, lastError: null });
    expect((await cmd.mnSharedSend(W, s.id, await grant(s.debit!))).session.stage).toEqual({ kind: "sent" });
  });

  it("refuses Send again once the registration was abandoned, before the grant", async () => {
    installFixture({ scenario: "sh-coordinator-sent-abandoned" });
    const [s] = await cmd.mnSharedSessions(W);
    const g = await grant(s.debit!);
    await expect(cmd.mnSharedSend(W, s.id, g)).rejects.toMatchObject({ code: "masternodes.shared_stage", data: { stage: "abandoned" } });
  });

  it("refuses Start, Change fee, Join, Lock and Approve on mainnet as the engine does, and discards and abandons there", async () => {
    const off = { code: "masternodes.unsupported", data: { kind: "unsupported", feature: "protx_sending", field: "mainnet" } };
    installFixture({ scenario: "sh-capability-active", network: "mainnet" });
    await expect(cmd.mnSharedStart(W, request())).rejects.toMatchObject(off);
    const sheet = async (scenario: ScenarioName) => {
      installFixture({ scenario, network: "mainnet" });
      return (await cmd.mnSharedSessions(W))[0]!;
    };
    let s = await sheet("sh-coordinator-draft");
    await expect(cmd.mnSharedSetFee(W, s.id, 200_000)).rejects.toMatchObject(off);
    await cmd.mnSharedDiscard(W, s.id);
    expect(await cmd.mnSharedSessions(W)).toEqual([]);
    s = await sheet("sh-participant-invited");
    await expect(cmd.mnSharedJoin(W, s.id, { shareIndex: null, reward: { kind: "refund" } })).rejects.toMatchObject(off);
    s = await sheet("sh-coordinator-ready");
    await expect(cmd.mnSharedLock(W, s.id, s.revision, await grant())).rejects.toMatchObject(off);
    s = await sheet("sh-participant-approve");
    await expect(cmd.mnSharedApprove(W, s.id, s.consentHash!, await grant())).rejects.toMatchObject(off);
    installFixture({ scenario: "sh-coordinator-ready" });
    s = (await cmd.mnSharedSessions(W))[0]!;
    expect((await cmd.mnSharedLock(W, s.id, s.revision, await grant())).session.stage).toEqual({ kind: "signing" });
  });

  it("refuses Sign and Send on mainnet before the engine, naming the network", async () => {
    for (const [scenario, call] of [
      ["sh-participant-combined-mainnet", cmd.mnSharedSign],
      ["sh-coordinator-send-mainnet", cmd.mnSharedSend],
    ] as const) {
      installFixture({ scenario });
      const [s] = await cmd.mnSharedSessions(W);
      await expect(call(W, s.id, "no-grant")).rejects.toMatchObject({ code: "app.shared_send_off", data: { kind: "sharedSendNetwork", network: "mainnet" } });
    }
  });
});
