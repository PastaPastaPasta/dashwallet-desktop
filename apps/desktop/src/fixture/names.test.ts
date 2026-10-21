// The fixture's contested names (names.ts, MG-11) through the generated commands: MG-07's record
// as dw-app converts it, the whole-call refusals in the facade's order, and Drive's counting (one
// choice per masternode and label, the weight moved, the same choice `already_voted`, the limit).

import { readFileSync } from "node:fs";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { installFixture } from "./index";
import { FIXTURE_NAMES, ID_1, ID_2, VOTER_A, VOTER_B, VOTER_C } from "./names";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";

const PASS = { kind: "passphrase", passphrase: FIXTURE_PASSPHRASE } as const;
const VECTORS = JSON.parse(readFileSync(new URL("./vectors/governance.json", import.meta.url), "utf8"));

const install = (scenario: ScenarioName) => installFixture({ scenario });
const grant = async () => (await cmd.authorize({ kind: "governance" }, W, PASS)).id;
const towards = (identity: string) => ({ kind: "towardsIdentity", identity }) as const;
const a11ce = async () => (await cmd.governanceContestedNames(W)).contests.find((c) => c.label === "a11ce")!;
const stateOf = async (voter: string, label: string) => (await cmd.governanceContestedNames(W)).voters.find((v) => v.proTxHash === voter)!.current.find((s) => s.label === label);

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  install("names");
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture contested names", () => {
  it("reads MG-07's fixture as dw-app converts it", async () => {
    expect(FIXTURE_NAMES).toEqual(VECTORS.results.governance_contested_names);
    expect(await cmd.governanceContestedNames(W)).toEqual(FIXTURE_NAMES);
    // Name votes need no governance sync (MG-07 D2).
    install("governance-disabled");
    expect((await cmd.governanceContestedNames(W)).contests).toHaveLength(2);
  });

  it("fails as a whole without the list or Platform", async () => {
    install("names-list-unavailable");
    await expect(cmd.governanceContestedNames(W)).rejects.toMatchObject({
      code: "governance.list_unavailable",
    });
    await expect(cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [VOTER_A], await grant())).rejects.toMatchObject({ code: "governance.list_unavailable" });
    install("names-platform-unavailable");
    await expect(cmd.governanceContestedNames(W)).rejects.toMatchObject({
      code: "platform.unavailable",
    });
  });

  it("refuses a whole call in the facade's order", async () => {
    const g = await grant();
    await expect(cmd.governanceVoteName(W, "Alice", { kind: "lock" }, [VOTER_A], g)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [], g)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [VOTER_A, VOTER_A], g)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.governanceVoteName(W, "b0b00", { kind: "lock" }, [VOTER_A], g)).rejects.toMatchObject({
      code: "governance.contest_not_found",
      data: { kind: "contestLabel", label: "b0b00" },
    });
    await expect(cmd.governanceVoteName(W, "dash-pay", towards(ID_1), [VOTER_A], g)).rejects.toMatchObject({
      code: "governance.not_a_contender",
      data: { kind: "contender", identity: ID_1 },
    });
    await expect(cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [VOTER_A], "no-such-grant")).rejects.toMatchObject({ code: "platform.grant_invalid" });
    install("names-ended");
    await expect(cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [VOTER_A], await grant())).rejects.toMatchObject({
      code: "governance.contest_closed",
      data: { kind: "contestLabel", label: "a11ce" },
    });
  });

  it("counts as Drive does: the weight moves, a repeat and the limit are refused", async () => {
    const casts = await cmd.governanceVoteName(W, "a11ce", towards(ID_2), [VOTER_A, VOTER_B, VOTER_C, "d4".repeat(32)], await grant());
    expect(casts.map((c) => [c.proTxHash, c.rejected])).toEqual([
      [VOTER_A, null],
      // At its 5 votes the engine sends nothing (MG-07 D6).
      [VOTER_B, "vote_limit"],
      [VOTER_C, null],
      ["d4".repeat(32), "unknown_masternode"],
    ]);
    expect(casts[0].transition).toMatch(/^[0-9a-f]{64}$/);
    expect(casts[1].transition).toBeNull();
    const c = await a11ce();
    // A moved from ID_1 (9 → 8); A and the new voter C add to ID_2 (4 → 6).
    expect(c.contenders.map((k) => k.votes)).toEqual([8, 6]);
    expect(await stateOf(VOTER_A, "a11ce")).toEqual({
      label: "a11ce",
      choice: towards(ID_2),
      votesUsed: 3,
    });
    expect(await stateOf(VOTER_C, "a11ce")).toEqual({
      label: "a11ce",
      choice: towards(ID_2),
      votesUsed: 1,
    });
    // The same choice again is Drive's refusal, after the broadcast.
    const again = await cmd.governanceVoteName(W, "a11ce", towards(ID_2), [VOTER_A], await grant());
    expect(again[0].rejected).toBe("already_voted");
    expect(again[0].transition).not.toBeNull();
    // B's present choice at its limit: `already_voted`, nothing sent.
    const lock = await cmd.governanceVoteName(W, "a11ce", { kind: "lock" }, [VOTER_B], await grant());
    expect(lock).toEqual([{ proTxHash: VOTER_B, transition: null, rejected: "already_voted" }]);
    expect((await a11ce()).lockVotes).toBe(5);
  });

  it("takes an unconfirmed vote that landed: the re-read shows it, the count does not grow", async () => {
    install("names-vote-mixed");
    const casts = await cmd.governanceVoteName(W, "a11ce", { kind: "abstain" }, [VOTER_A, VOTER_C], await grant());
    expect(casts.map((c) => c.rejected)).toEqual(["unconfirmed", "masternode_not_found"]);
    expect(await stateOf(VOTER_A, "a11ce")).toEqual({
      label: "a11ce",
      choice: { kind: "abstain" },
      votesUsed: 2,
    });
    expect(await stateOf(VOTER_C, "a11ce")).toBeUndefined();
    expect((await a11ce()).abstainVotes).toBe(1);
  });

  it("reads in the facade's order, and goes down after a vote in names-check-fails", async () => {
    install("names-list-unavailable");
    await expect(cmd.governanceContestedNames("no-such-wallet")).rejects.toMatchObject({ code: "governance.list_unavailable" });
    install("names-platform-unavailable");
    await expect(cmd.governanceContestedNames("no-such-wallet")).rejects.toMatchObject({ code: "wallet_not_found" });
    install("names-check-fails");
    const casts = await cmd.governanceVoteName(W, "a11ce", { kind: "abstain" }, [VOTER_A], await grant());
    expect(casts.map((c) => c.rejected)).toEqual(["unconfirmed"]);
    await expect(cmd.governanceContestedNames(W)).rejects.toMatchObject({ code: "platform.unavailable" });
  });

  it("builds every names scenario", async () => {
    for (const s of ["names-empty", "names-no-voters", "names-ending", "names-ended", "names-partial", "names-watch-only", "names-check-fails", "names-still-unconfirmed"] as ScenarioName[]) {
      install(s);
      await cmd.governanceContestedNames(W);
    }
    install("names-empty");
    expect((await cmd.governanceContestedNames(W)).contests).toEqual([]);
    install("names-partial");
    expect((await a11ce()).contendersComplete).toBe(false);
    install("names-watch-only");
    expect((await cmd.listWallets())[0].watchOnly).toBe(true);
  });
});
