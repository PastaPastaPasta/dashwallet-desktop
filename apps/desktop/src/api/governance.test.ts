// The governance binding's contract (BL-232). The generated commands are held against
// docs/contracts/mg-engine.md: each §2 call is a `governance_*` command or is left out for a
// stated reason, and each §4 code is in the error union. Then dw-app's own JSON
// (fixture/vectors/governance.json, which rust/crates/dw-app/src/governance_vectors_tests.rs
// writes from engine records and fails on when it drifts) is held against the §3 records: each
// command returns what its §3 signature says, and each record carries exactly its contract
// fields, camelCased, each of its §3 type's JSON type, an enum's value one of its camelCased
// variants. Every wrapper is called once, for the shape of its arguments.

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

/** The §3 records' fields, in snake case with their Rust types, by record name. Every
 * `pub struct` with a body must parse, so a format change fails here, not by skipping a record. */
function contractRecords(): Map<string, [string, string][]> {
  const surface = section("## 3. Surface");
  const records = new Map<string, [string, string][]>();
  for (const m of surface.matchAll(/^pub struct (\w+) \{\n((?:    pub .*\n)*)\}/gm)) {
    records.set(m[1], [...m[2].matchAll(/^    pub (\w+): (.*),$/gm)].map((f) => [f[1], f[2]]));
  }
  const declared = [...surface.matchAll(/^pub struct (\w+) \{\n(?! {4}\.\.\n)/gm)].map((m) => m[1]);
  expect([...records.keys()].sort()).toEqual(declared.sort());
  return records;
}

/** The §3 enums of unit variants, by name, as their camelCase JSON values. */
function contractEnums(): Map<string, string[]> {
  const enums = new Map<string, string[]>();
  for (const m of section("## 3. Surface").matchAll(/^pub enum (\w+) \{\n((?: {4}\w+,\n)*)\}/gm)) {
    enums.set(m[1], [...m[2].matchAll(/^ {4}(\w+),$/gm)].map((v) => v[1][0].toLowerCase() + v[1].slice(1)));
  }
  return enums;
}

/** The §3 Rust types that cross as JSON numbers, strings and booleans. */
const JSON_TYPES: Record<string, string> = { u32: "number", u64: "number", i64: "number", f64: "number", String: "string", bool: "boolean" };

interface Contract {
  records: Map<string, [string, string][]>;
  enums: Map<string, string[]>;
}

/** What each facade method of §3 returns: the type inside `Result<…>`, and whether it is a `Vec`. */
function contractReturns(): Map<string, { type: string; list: boolean }> {
  const returns = new Map<string, { type: string; list: boolean }>();
  const signatures = section("## 3. Surface").matchAll(/^ {4}pub (?:async )?fn (\w+)\(.*\) -> Result<(Vec<)?(\w+|\(\))>?, GovernanceError>;$/gm);
  for (const m of signatures) returns.set(m[1], { type: m[3], list: m[2] !== undefined });
  return returns;
}

const camel = (s: string) => s.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/** Checks that `value` has exactly `record`'s fields, each of its §3 type (a record nested in
 * it checked the same way), adding each record it checks to `seen`. */
function expectRecord(c: Contract, record: string, value: unknown, at: string, seen: Set<string>) {
  const fields = c.records.get(record);
  expect(fields, `${at}: ${record} in §3`).toBeDefined();
  expect(Object.keys(value as object).sort(), at).toEqual(fields!.map(([f]) => camel(f)).sort());
  seen.add(record);
  for (const [field, type] of fields!) {
    expectValue(c, type, (value as Record<string, unknown>)[camel(field)], `${at}.${camel(field)}`, seen);
  }
}

/** Checks that `value` is of the §3 type `type`: `null` for a `None`, an array for a `Vec`. */
function expectValue(c: Contract, type: string, value: unknown, at: string, seen: Set<string>) {
  const [, option, list, leaf] = type.match(/^(Option<)?(Vec<)?(\w+)>?>?$/) ?? [];
  expect(leaf, `${at}: ${type} parses`).toBeDefined();
  if (option && value === null) return;
  expect(Array.isArray(value), at).toBe(list !== undefined);
  for (const [i, v] of (list ? (value as unknown[]) : [value]).entries()) {
    const there = list ? `${at}[${i}]` : at;
    if (c.records.has(leaf)) expectRecord(c, leaf, v, there, seen);
    else if (c.enums.has(leaf)) expect(c.enums.get(leaf), `${there}: ${leaf}`).toContain(v);
    else {
      expect(JSON_TYPES[leaf], `${there}: ${leaf} is a §3 record, enum or JSON leaf`).toBeDefined();
      expect(typeof v, `${there}: ${leaf}`).toBe(JSON_TYPES[leaf]);
      if (leaf !== "f64" && JSON_TYPES[leaf] === "number") expect(Number.isInteger(v), `${there}: ${leaf}`).toBe(true);
    }
  }
}

/** §3 records no bound command returns: the `proposals` argument and the unbound calls' (D93). */
const NOT_RETURNED = ["ProposalQuery", "RawVote", "VoteRelayed"];

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

  it("mirrors each §3 enum of a record field, variant for variant", () => {
    const c: Contract = { records: contractRecords(), enums: contractEnums() };
    const used = new Set([...c.records.values()].flat().map(([, t]) => t.match(/(\w+)>*$/)![1]).filter((t) => !(t in JSON_TYPES) && !c.records.has(t)));
    expect([...used].sort()).toEqual(["GovernanceSyncPhase", "MasternodeType", "ProposalScope", "ProposalStatus", "VoteOutcome", "VoteSignal"]);
    for (const e of used) {
      // dw-app's mirror, `<Enum>Name`, as ts-rs writes it.
      const ts = readFileSync(new URL(`./generated/${e}Name.ts`, import.meta.url), "utf8");
      const union = [...(ts.match(/^export type \w+ = (.*);$/m)?.[1] ?? "").matchAll(/"(\w+)"/g)].map((m) => m[1]);
      expect(union, e).toEqual(c.enums.get(e));
    }
  });

  it("returns exactly the §3 records, camelCased and typed", () => {
    const c: Contract = { records: contractRecords(), enums: contractEnums() };
    const { records } = c;
    const returns = contractReturns();
    const seen = new Set<string>();
    const r = VECTORS.results;
    for (const [name, value] of Object.entries(r) as [GovernanceCommand, unknown][]) {
      const ret = returns.get(name.replace(/^governance_/, ""));
      expect(ret, name).toBeDefined();
      const { type, list } = ret!;
      expect(Array.isArray(value), name).toBe(list);
      if (type === "()") expect(value, name).toBeNull();
      else if (type === "bool") expect(typeof value, name).toBe("boolean");
      else {
        const values = list ? (value as unknown[]) : [value];
        expect(values.length, name).toBeGreaterThan(0);
        for (const [i, v] of values.entries()) expectRecord(c, type, v, `${name}[${i}]`, seen);
      }
    }
    // Every record a bound command can return was checked, none skipped for being absent.
    expect([...seen].sort()).toEqual([...records.keys()].filter((k) => !NOT_RETURNED.includes(k)).sort());
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
