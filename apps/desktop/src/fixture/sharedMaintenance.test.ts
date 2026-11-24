// The fixture's shared-masternode maintenance (shared.ts, MG-10 §2.5–§2.8, S6) through the generated
// commands: refusals mirror the engine's (grant cap, stage, the key-change send) and MG's mainnet
// ruling (sends refused, standby create and export allowed).

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { installFixture } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { SH_MN } from "./shared";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const COIN = 100_000_000;
const grant = async (maxDuffs: number) => (await cmd.authorize({ kind: "masternodeOp", maxDuffs }, W, PASS)).id;
const together = async () => (await cmd.mnSharedSessions(W)).find((s) => s.kind === "dissolve_together")!;
const keyChange = async () => (await cmd.mnSharedSessions(W)).find((s) => s.kind === "key_change")!;

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture shared-masternode maintenance", () => {
  it("previews a dissolution alone and sends it within the preview's total debit", async () => {
    installFixture({ scenario: "sh-detail" });
    const request = { proTxHash: SH_MN, shareIndex: 0, fee: 1_000 };
    const p = await cmd.mnSharedPreviewDissolve(W, request);
    expect(p).toMatchObject({ early: true, penalty: 20 * COIN, fee: 1_000, totalDebit: 20 * COIN + 1_000 });
    await expect(cmd.mnSharedPreviewDissolve(W, { ...request, shareIndex: 1 })).rejects.toMatchObject({ code: expect.stringMatching(/^masternodes\./) });
    await expect(cmd.mnSharedDissolve(W, request, await grant(p.totalDebit - 1))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    expect(await cmd.mnSharedDissolve(W, request, await grant(p.totalDebit))).toMatchObject({ debit: p.totalDebit });
  });

  it("creates standbys within the penalty and fee, exports one, and sends the immediate variant", async () => {
    installFixture({ scenario: "sh-standby-none" });
    expect(await cmd.mnSharedStandby(W, SH_MN)).toEqual([]);
    await expect(cmd.mnSharedCreateStandby(W, SH_MN, 1_000, await grant(20 * COIN))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const made = await cmd.mnSharedCreateStandby(W, SH_MN, 1_000, await grant(20 * COIN + 1_000));
    expect(made).toMatchObject([{ shareIndex: 0, fee: 1_000, full: true, immediate: true, penalty: 20 * COIN }]);
    expect(await cmd.mnSharedExportStandby(W, SH_MN, 0, 1_000, await grant(0))).toBe(`standby-dissolution-${SH_MN.slice(0, 8)}-share1.txt`);
    await expect(cmd.mnSharedSendStandby(W, SH_MN, 0, 1_000, "full", await grant(1_000))).rejects.toMatchObject({ code: "masternodes.shared_too_early" });
    await expect(cmd.mnSharedSendStandby(W, SH_MN, 0, 1_000, "immediate", await grant(1_000))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    expect(await cmd.mnSharedSendStandby(W, SH_MN, 0, 1_000, "immediate", await grant(20 * COIN + 1_000))).toMatchObject({ debit: 20 * COIN + 1_000 });
  });

  it("a cancelled save exports nothing", async () => {
    installFixture({ scenario: "sh-standby-export-cancel" });
    expect(await cmd.mnSharedExportStandby(W, SH_MN, 0, 1_000, await grant(0))).toBeNull();
  });

  it("approves a dissolution together, its actor fee only once accepted, then sends it", async () => {
    installFixture({ scenario: "sh-together-actor-fee" });
    const s = await together();
    await expect(cmd.mnSharedApproveMaintenance(W, s.id, await grant(1_000), false)).rejects.toMatchObject({ code: "masternodes.shared_actor_fee", data: { kind: "sharedActorFee", fee: 1_000 } });
    await expect(cmd.mnSharedApproveMaintenance(W, s.id, await grant(0), true)).rejects.toMatchObject({ code: "send.grant_exceeded" });
    const out = await cmd.mnSharedApproveMaintenance(W, s.id, await grant(1_000), true);
    expect(out.session).toMatchObject({ stage: { kind: "combined" }, next: "send" });
    const sent = await cmd.mnSharedSendMaintenance(W, s.id, await grant(0));
    expect(sent.kind).toBe("operation");
    expect(await together()).toMatchObject({ stage: { kind: "sent" }, discardable: false });
  });

  it("refuses to send a key change, before anything else", async () => {
    installFixture({ scenario: "sh-keychange-combined" });
    const s = await keyChange();
    await expect(cmd.mnSharedSendMaintenance(W, s.id, await grant(0))).rejects.toMatchObject({ code: "app.shared_send_unsupported", data: { kind: "sharedUnsupported", sessionKind: "key_change" } });
    expect(await keyChange()).toMatchObject({ stage: { kind: "combined" }, discardable: true });
    await cmd.mnSharedDiscard(W, s.id);
    expect(await keyChange()).toBeUndefined();
  });

  it("starts a key change and a dissolution together, each charged its fee or nothing", async () => {
    installFixture({ scenario: "sh-detail" });
    const kc = await cmd.mnSharedStartKeyChange(W, { proTxHash: SH_MN, operator: { kind: "keep" }, voting: { kind: "next_unused" } }, await grant(0));
    expect(kc.session).toMatchObject({ kind: "key_change", role: "coordinator", discardable: true, keyChange: { funder: true } });
    await expect(cmd.mnSharedStartKeyChange(W, { proTxHash: SH_MN, operator: { kind: "keep" }, voting: { kind: "keep" } }, await grant(0))).rejects.toMatchObject({ code: "invalid_argument" });
    const request = { proTxHash: SH_MN, shareIndex: 0, fee: 1_000 };
    await expect(cmd.mnSharedStartDissolveTogether(W, request, await grant(999))).rejects.toMatchObject({ code: "send.grant_exceeded" });
    expect((await cmd.mnSharedStartDissolveTogether(W, request, await grant(1_000))).session).toMatchObject({ kind: "dissolve_together", stage: { kind: "signing" } });
  });

  it.each<ScenarioName>(["sh-detail-mainnet", "sh-together-combined-mainnet"])("%s: every send refused, create, export and approvals allowed", async (scenario) => {
    installFixture({ scenario });
    const off = { code: "app.shared_send_off", data: { kind: "sharedSendNetwork", network: "mainnet" } };
    const request = { proTxHash: SH_MN, shareIndex: 0, fee: 1_000 };
    await expect(cmd.mnSharedDissolve(W, request, await grant(30 * COIN))).rejects.toMatchObject(off);
    await expect(cmd.mnSharedSendStandby(W, SH_MN, 0, 1_000, "immediate", await grant(30 * COIN))).rejects.toMatchObject(off);
    const s = await together();
    if (s) await expect(cmd.mnSharedSendMaintenance(W, s.id, await grant(0))).rejects.toMatchObject(off);
    expect(await cmd.mnSharedPreviewDissolve(W, request)).toMatchObject({ totalDebit: 20 * COIN + 1_000 });
    expect(await cmd.mnSharedExportStandby(W, SH_MN, 0, 1_000, await grant(0))).toMatch(/^standby-dissolution-/);
    expect(await cmd.mnSharedCreateStandby(W, SH_MN, 1_000, await grant(20 * COIN + 1_000))).toHaveLength(1);
    expect((await cmd.mnSharedStartDissolveTogether(W, request, await grant(1_000))).session.kind).toBe("dissolve_together");
  });
});
