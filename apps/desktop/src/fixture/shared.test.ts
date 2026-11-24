// The fixture's shared masternodes (shared.ts) through the generated commands: a coordinator's
// session from start to locked terms, a participant's join, and Discard only while `discardable`.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { StartSharedRequest } from "../api/generated/StartSharedRequest";
import { installFixture } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W } from "./scenarios";
import { SH_TEXT } from "./shared";

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

const grant = async () => (await cmd.authorize({ kind: "masternodeOp", maxDuffs: 0 }, W, PASS)).id;

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
});
