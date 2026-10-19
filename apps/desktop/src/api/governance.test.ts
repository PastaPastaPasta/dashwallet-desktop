// The governance binding's contract (BL-232). The generated commands are held against
// docs/contracts/mg-engine.md: each §2 call is a `governance_*` command or is left out for a
// stated reason, and each §4 code is in the error union. Then dw-app's own JSON
// (fixture/vectors/governance.json, which rust/crates/dw-app/src/governance_vectors_tests.rs
// writes from engine records and fails on when it drifts) is held against the §3 records: each
// record carries exactly its contract fields, camelCased. Every wrapper is called once, for the
// shape of its arguments.

import { readFileSync } from "node:fs";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "./generated/commands";
import type { AppError, CommandName, CommandResults } from "./generated/commands";
import { ERROR_CLASSES } from "../fixture/errors";

type GovernanceCommand = Extract<CommandName, `governance_${string}`>;

interface Vectors {
  results: { [K in GovernanceCommand]: CommandResults[K] };
  errors: AppError[];
}

const CONTRACT = readFileSync(new URL("../../../../docs/contracts/mg-engine.md", import.meta.url), "utf8");
const VECTORS: Vectors = JSON.parse(readFileSync(new URL("../fixture/vectors/governance.json", import.meta.url), "utf8"));

/** mg-engine §2 calls with no command, and why (dw-app's `api/governance.rs` EXCLUDED, MG-02 D93). */
const NOT_BOUND: Record<string, string> = {
  governance: "the handle: every command takes it from the open session",
  superblock_budget: "a free function of a height; governance_upcoming_superblocks gives each budget",
  relay_vote: "Core's voteraw, which the console runs",
  vote: "a stub until MG-05's voting grant",
};

/** The text of the `##`-level section that starts with `heading`. */
function section(heading: string): string {
  const start = CONTRACT.indexOf(`\n${heading}`);
  expect(start, heading).toBeGreaterThan(0);
  const end = CONTRACT.indexOf("\n## ", start + 1);
  return CONTRACT.slice(start, end < 0 ? undefined : end);
}

/** The calls the §2 tables name, in snake case. */
function contractCalls(): string[] {
  return [...section("## 2. Calls").matchAll(/^\| `(?:NetworkSession\.)?(\w+)\(/gm)].map((m) => m[1]);
}

/** The §3 records' fields, in snake case with their Rust types, by record name. */
function contractRecords(): Map<string, [string, string][]> {
  const records = new Map<string, [string, string][]>();
  for (const m of section("## 3. Surface").matchAll(/^pub struct (\w+) \{\n((?:    pub .*\n)*)\}/gm)) {
    records.set(m[1], [...m[2].matchAll(/^    pub (\w+): (.*),$/gm)].map((f) => [f[1], f[2]]));
  }
  return records;
}

const camel = (s: string) => s.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/** Checks that `value` has exactly `record`'s fields, then the §3 records nested in it. */
function expectRecord(records: Map<string, [string, string][]>, record: string, value: unknown, at: string) {
  const fields = records.get(record);
  expect(fields, `${at}: ${record} in §3`).toBeDefined();
  expect(Object.keys(value as object).sort(), at).toEqual(fields!.map(([f]) => camel(f)).sort());
  for (const [field, type] of fields!) {
    const nested = type.match(/^(?:Option<|Vec<)?(\w+)>?$/)?.[1];
    const inner = (value as Record<string, unknown>)[camel(field)];
    if (!nested || !records.has(nested) || inner == null) continue;
    for (const [i, v] of (Array.isArray(inner) ? inner : [inner]).entries()) {
      expectRecord(records, nested, v, `${at}.${camel(field)}[${i}]`);
    }
  }
}

/** The §3 record each command returns (a list of it where the call returns a `Vec`). */
const RETURNS: { [K in GovernanceCommand]: string | null } = {
  governance_sync_enabled: null,
  governance_set_sync_enabled: null,
  governance_sync_state: "GovernanceSyncState",
  governance_resync: null,
  governance_clock: "GovernanceClock",
  governance_info: "GovernanceInfo",
  governance_upcoming_superblocks: "SuperblockDate",
  governance_proposals: "ProposalRow",
  governance_proposal: "ProposalDetail",
  governance_current_votes: "CurrentVoteRow",
  governance_voting_masternodes: "VotingMasternode",
};

/** The codes §4 lists for `GovernanceError`. */
function contractCodes(): string[] {
  const row = section("## 4. Error codes")
    .split("\n")
    .find((l) => l.startsWith("| `GovernanceError` |"));
  expect(row).toBeDefined();
  return [...row!.matchAll(/`(governance\.\w+)`/g)].map((m) => m[1]);
}

const governanceCommands = () => Object.entries(cmd.COMMANDS).filter(([, domain]) => domain === "governance").map(([name]) => name);

const calls: [string, unknown][] = [];

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  calls.length = 0;
});

afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("governance contract", () => {
  it("binds every mg-engine call or says why not", () => {
    const listed = contractCalls();
    expect(listed).toHaveLength(15);
    const bound = listed.filter((c) => !(c in NOT_BOUND)).map((c) => `governance_${c}`);
    expect(governanceCommands().sort()).toEqual(bound.sort());
    for (const c of Object.keys(NOT_BOUND)) expect(listed, c).toContain(c);
    expect(Object.keys(VECTORS.results).sort()).toEqual(bound.sort());
  });

  it("knows every governance code of mg-engine §4", () => {
    const codes = contractCodes();
    expect(codes).toHaveLength(7);
    const union = cmd.ERROR_CODES.filter((c) => c.startsWith("governance."));
    expect([...union].sort()).toEqual([...codes].sort());
    // dw-app's own mapping reaches each of them, with an ErrorData class the fixture can inject.
    const reached = VECTORS.errors.map((e) => e.code);
    for (const c of codes) expect(reached, c).toContain(c);
    for (const e of VECTORS.errors) {
      expect(cmd.ERROR_CODES, e.code).toContain(e.code);
      expect(ERROR_CLASSES, e.code).toContain(e.data?.kind ?? "none");
    }
  });

  it("calls every wrapper with camelCase arguments", async () => {
    mockIPC((name, payload) => {
      calls.push([name, payload]);
      return VECTORS.results[name as GovernanceCommand];
    });
    const r = VECTORS.results;
    const hash = r.governance_proposal.row.hash;
    const outpoint = r.governance_current_votes[1].outpoint;
    // Typed by command, so a command added without a call here fails tsc.
    const run: { [K in GovernanceCommand]: () => Promise<CommandResults[K]> } = {
      governance_sync_enabled: () => cmd.governanceSyncEnabled(),
      governance_set_sync_enabled: () => cmd.governanceSetSyncEnabled(true),
      governance_sync_state: () => cmd.governanceSyncState(),
      governance_resync: () => cmd.governanceResync(),
      governance_clock: () => cmd.governanceClock(),
      governance_info: () => cmd.governanceInfo(),
      governance_upcoming_superblocks: () => cmd.governanceUpcomingSuperblocks(2),
      governance_proposals: () => cmd.governanceProposals({ scope: "current", text: "wallet" }),
      governance_proposal: () => cmd.governanceProposal(hash),
      governance_current_votes: () => cmd.governanceCurrentVotes(hash, outpoint),
      governance_voting_masternodes: () => cmd.governanceVotingMasternodes("ab".repeat(32), hash),
    };
    for (const call of Object.values(run)) await call();
    expect(calls.map(([name]) => name).sort()).toEqual(Object.keys(run).sort());
    expect(calls).toContainEqual(["governance_set_sync_enabled", { enabled: true }]);
    expect(calls).toContainEqual(["governance_upcoming_superblocks", { count: 2 }]);
    expect(calls).toContainEqual(["governance_proposals", { query: { scope: "current", text: "wallet" } }]);
    expect(calls).toContainEqual(["governance_current_votes", { hash, outpoint }]);
    expect(calls).toContainEqual(["governance_voting_masternodes", { walletId: "ab".repeat(32), proposal: hash }]);
  });

  it("returns exactly the §3 records, camelCased", () => {
    const records = contractRecords();
    const r = VECTORS.results;
    for (const [name, record] of Object.entries(RETURNS) as [GovernanceCommand, string | null][]) {
      const value = r[name];
      if (record === null) {
        expect(typeof value === "boolean" || value === null, name).toBe(true);
        continue;
      }
      const values = Array.isArray(value) ? value : [value];
      expect(values.length, name).toBeGreaterThan(0);
      for (const [i, v] of values.entries()) expectRecord(records, record, v, `${name}[${i}]`);
    }
    // The fields BL-231 added reach the renderer under their camelCase names (MG-02 D82, D84).
    expect(r.governance_proposal.signals.delete.no.estimate).toBe(2402);
    expect(r.governance_proposal.deletionMarkedAt).toBe(1_790_000_600);
    expect(r.governance_current_votes.map((v) => v.signal)).toEqual(["funding", "delete"]);
    // A shared key's vote is the key's, not each masternode's (mg-engine §2.4).
    const shared = r.governance_voting_masternodes.find((m) => m.keyHolders > 1);
    expect([shared?.currentVote, shared?.keyLastVote]).toEqual([null, "abstain"]);
    const notSynced = VECTORS.errors.find((e) => e.code === "governance.not_synced");
    expect(notSynced?.data).toEqual({ kind: "governancePhase", phase: "syncingVotes" });
  });
});
