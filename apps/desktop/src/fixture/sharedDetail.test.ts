// The fixture's confirmed shared masternode (shared.ts, MG-10 §2.4, §5.1, S6) through the generated
// commands: EP 5's shares and the stored standbys carry exactly their generated types' fields, the
// row reads as the wallet's shared masternode, and the reads refuse as the engine does.

import { readFileSync } from "node:fs";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { registrationSession } from "../masternodes/shared";
import { installFixture } from "./index";
import { FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { SH_COORD, SH_MN, SH_REGISTERED } from "./shared";

const DETAIL: ScenarioName[] = ["sh-detail", "sh-detail-sessionless", "sh-detail-too-early", "sh-detail-list-unavailable", "sh-detail-upgrade-unknown", "sh-detail-standby-valid"];

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

function fieldsOf(name: string): string[] {
  const text = readFileSync(new URL(`../api/generated/${name}.ts`, import.meta.url), "utf8").replace(/\/\*\*[\s\S]*?\*\//g, "");
  const body = text.slice(text.indexOf("= {") + 3, text.lastIndexOf("}"));
  return [...body.matchAll(/(?:^|,)\s*(\w+):/g)].map((m) => m[1]).sort();
}

const expectShape = (name: string, v: object) => expect(Object.keys(v).sort(), name).toEqual(fieldsOf(name));

describe("fixture shared masternode", () => {
  it.each(DETAIL)("%s: EP 5's shares and the standbys have their generated shapes", async (scenario) => {
    installFixture({ scenario });
    const s = await cmd.mnSharedShares(W, SH_MN);
    expectShape("SharedShares", s);
    for (const f of s.shares) {
      expectShape("ShareFact", f);
      expectShape("RewardDestination", f.reward);
      if (f.reward.latestConfirmedLocal) expectShape("RewardUpdate", f.reward.latestConfirmedLocal);
      if (f.reward.latestAttempt) expectShape("RewardAttempt", f.reward.latestAttempt);
    }
    expect(s.shares.map((f) => [f.amount / 100_000_000, f.mine])).toEqual([
      [400, true],
      [300, false],
      [300, false],
    ]);
    expect(s.fullValidFrom).toBe(s.registeredHeight + s.earlyPeriodBlocks);
    expect(s.earlyPenalty).toBe(20 * 100_000_000);
    for (const sb of await cmd.mnSharedStandby(W, SH_MN)) expectShape("Standby", sb);
    // Fixture data stays testnet-shaped.
    for (const f of s.shares) expect(f.refundAddress).toMatch(/^y/);
  });

  it.each([...DETAIL, "sh-detail-unknown" as const])("%s: the row is the wallet's shared masternode, first in the list, its payouts refused", async (scenario) => {
    installFixture({ scenario });
    const o = await cmd.mnList(W);
    expect(o.masternodes[0]).toMatchObject({ proTxHash: SH_MN, mine: true, sharedUnsupported: true, shares: { held: 1, of: 3 } });
    expect((await cmd.mnMasternode(W, SH_MN)).registrar).toEqual({ kind: "shared" });
    await expect(cmd.mnPayouts(W, SH_MN, null)).rejects.toMatchObject({ code: "masternodes.unsupported" });
  });

  it("sh-detail: the confirmed registration session labels the shares; a confirmed change and a sent one", async () => {
    installFixture({ scenario: "sh-detail" });
    const s = await cmd.mnSharedShares(W, SH_MN);
    expect(s).toMatchObject({ registeredHeight: SH_REGISTERED, earlyPeriodBlocks: 105_120, fullValidFrom: 1_319_400, eligibility: "ok", fullValidNow: false, session: SH_COORD });
    const session = registrationSession(await cmd.mnSharedSessions(W), SH_MN, s.session);
    expect(session).toMatchObject({ id: SH_COORD, kind: "registration", stage: { kind: "confirmed", height: SH_REGISTERED }, proTxHash: SH_MN });
    expect(session!.rows.map((r) => r.label)).toEqual(["Alice", "Bob", "Carol"]);
    expect(s.shares[0].reward.latestConfirmedLocal).not.toBeNull();
    expect(s.shares[0].reward.latestAttempt).toMatchObject({ state: "sent" });
    expect(await cmd.mnSharedStandby(W, SH_MN)).toMatchObject([{ shareIndex: 0, full: true, immediate: true, source: "created", fullValidFrom: 1_319_400 }]);
    expect((await cmd.mnMasternode(W, SH_MN)).registration?.registeredHeight).toBe(SH_REGISTERED);
  });

  it("sh-detail-sessionless: EP 5 answers from history with no session and no standby", async () => {
    installFixture({ scenario: "sh-detail-sessionless" });
    expect((await cmd.mnSharedShares(W, SH_MN)).session).toBeNull();
    expect(registrationSession(await cmd.mnSharedSessions(W), SH_MN)).toBeNull();
    expect(await cmd.mnSharedStandby(W, SH_MN)).toEqual([]);
  });

  it("sh-detail-unknown: EP 5 refuses shared_registration_unknown", async () => {
    installFixture({ scenario: "sh-detail-unknown" });
    await expect(cmd.mnSharedShares(W, SH_MN)).rejects.toMatchObject({ code: "masternodes.shared_registration_unknown" });
    expect(await cmd.mnSharedStandby(W, SH_MN)).toEqual([]);
  });

  it("eligibility scenarios report their eligibility; upgrade-unknown with v24 unknown", async () => {
    for (const [scenario, eligibility] of [
      ["sh-detail-too-early", "too_early"],
      ["sh-detail-list-unavailable", "list_unavailable"],
      ["sh-detail-upgrade-unknown", "upgrade_unknown"],
    ] as const) {
      installFixture({ scenario });
      expect((await cmd.mnSharedShares(W, SH_MN)).eligibility, scenario).toBe(eligibility);
    }
    expect((await cmd.mnSharedCapability(W)).v24).toBe("unknown");
  });

  it("sh-detail-standby-valid: past the early period the full refund is valid now; Bob's standby imported", async () => {
    installFixture({ scenario: "sh-detail-standby-valid" });
    const s = await cmd.mnSharedShares(W, SH_MN);
    expect(s.fullValidNow).toBe(true);
    expect(s.fullValidFrom).toBeLessThan(s.spendHeight);
    expect((await cmd.mnSharedStandby(W, SH_MN)).map((x) => [x.shareIndex, x.source, x.full, x.immediate])).toEqual([
      [0, "created", true, true],
      [1, "imported", true, false],
    ]);
  });

  it("refuses a malformed hash and reads another masternode as unknown", async () => {
    installFixture({ scenario: "sh-detail" });
    await expect(cmd.mnSharedShares(W, "ec")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnSharedStandby(W, "ec")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.mnSharedShares(W, "4e".repeat(32))).rejects.toMatchObject({ code: "masternodes.shared_registration_unknown" });
    expect(await cmd.mnSharedShares(W, SH_MN.toUpperCase())).toMatchObject({ session: SH_COORD });
  });
});
