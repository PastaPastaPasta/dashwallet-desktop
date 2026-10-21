// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's contested DPNS names (MG-11 over MG-07 §1): what `contested_names` reads is MG-07's
// fixture (two contests, three voters with mixed keys and choices) as dw-app converts it, held
// equal to vectors/governance.json by names.test.ts (dw-app's governance_vectors test writes it
// from rust/crates/dw-engine/tests/fixtures/mg07). `vote_name` refuses as the facade does (its
// whole-call order, MG-07 §3) and counts a vote as Drive does (one choice per masternode and
// label, its weight moved from the old choice, the same choice again `already_voted`, at most
// `votesAllowed` votes). A scenario changes values in that record, never its shape.

import type { AppError } from "../api/generated/commands";
import type { ContestedName } from "../api/generated/ContestedName";
import type { ContestedNames } from "../api/generated/ContestedNames";
import type { NameChoice } from "../api/generated/NameChoice";
import type { NameVoteCast } from "../api/generated/NameVoteCast";
import type { ThemeName } from "../api/generated/ThemeName";
import { sameChoice } from "../governance/names";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

const NOW = secs(FIXTURE_NOW);
const HEX64 = /^[0-9a-f]{64}$/;
/** DPNS's contested normalized label (MG-07 §2): 3–19 of `a-hj-km-np-z01-`, no `-` at an end. */
const LABEL = /^[a-hj-km-np-z01][a-hj-km-np-z01-]{1,17}[a-hj-km-np-z01]$/;
const BASE58 = /^[1-9A-HJ-NP-Za-km-z]{43,44}$/;

/** The two contenders of MG-07's fixture. */
export const ID_1 = "8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P";
export const ID_2 = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";
/** The voters: a regular masternode, an evonode, a PoSe-banned masternode with an attached key
 * (the governance fixture's MN_A–MN_C). */
export const VOTER_A = "a1".repeat(32);
export const VOTER_B = "b2".repeat(32);
export const VOTER_C = "c3".repeat(32);

/** MG-07's `contested_names.json` through dw-app's conversion. */
export const FIXTURE_NAMES: ContestedNames = {
  contests: [
    {
      label: "a11ce",
      endsAt: 1_800_003_600,
      contenders: [
        { identity: ID_1, votes: 9 },
        { identity: ID_2, votes: 4 },
      ],
      lockVotes: 1,
      abstainVotes: 0,
      contendersComplete: true,
    },
    {
      label: "dash-pay",
      endsAt: 1_800_090_000,
      contenders: [{ identity: ID_2, votes: 0 }],
      lockVotes: 0,
      abstainVotes: 0,
      contendersComplete: true,
    },
  ],
  voters: [
    {
      proTxHash: VOTER_A,
      kind: "regular",
      weight: 1,
      valid: true,
      key: { kind: "keychain", index: 0 },
      current: [
        {
          label: "a11ce",
          choice: { kind: "towardsIdentity", identity: ID_1 },
          votesUsed: 2,
        },
      ],
    },
    {
      proTxHash: VOTER_B,
      kind: "evonode",
      weight: 4,
      valid: true,
      key: { kind: "keychain", index: 3 },
      current: [
        { label: "a11ce", choice: { kind: "lock" }, votesUsed: 5 },
        { label: "dash-pay", choice: { kind: "abstain" }, votesUsed: 1 },
      ],
    },
    {
      proTxHash: VOTER_C,
      kind: "regular",
      weight: 1,
      valid: false,
      key: { kind: "attached" },
      current: [],
    },
  ],
  votesAllowed: 5,
};

export interface NamesWorld {
  /** What `contested_names` answers (the open contests are those whose `endsAt` it read ahead). */
  names: ContestedNames;
  /** Both calls fail as a whole: SPV has no masternode list, or Platform can't be reached. */
  unavailable: "list" | "platform" | null;
  /** `vote_name` answers these masternodes with this `rejected` reason instead of counting. */
  refuse: Record<string, string>;
  /** An `unconfirmed` answer whose vote Platform did take: the next read shows it (MG-07 D7). */
  unconfirmedLands: boolean;
}

export const newNamesWorld = (): NamesWorld => ({
  names: structuredClone(FIXTURE_NAMES),
  unavailable: null,
  refuse: {},
  unconfirmedLands: false,
});

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** A stable 64-hex "transition hash" (FNV-1a, repeated). */
function transitionHash(text: string): string {
  let x = 0x811c9dc5;
  for (const ch of text) x = Math.imul(x ^ ch.charCodeAt(0), 0x01000193) >>> 0;
  return x.toString(16).padStart(8, "0").repeat(8);
}

function readable(n: NamesWorld) {
  if (n.unavailable === "list") throw fail("governance.list_unavailable", "masternode list unavailable");
  if (n.unavailable === "platform") throw fail("platform.unavailable", "fixture: Platform can't be reached");
}

/** Moves `weight` from the voter's old choice to its new one in the contest's tally. */
function move(c: ContestedName, from: NameChoice | null, to: NameChoice, weight: number) {
  const add = (choice: NameChoice, by: number) => {
    if (choice.kind === "lock") c.lockVotes = (c.lockVotes ?? 0) + by;
    else if (choice.kind === "abstain") c.abstainVotes = (c.abstainVotes ?? 0) + by;
    else {
      const x = c.contenders.find((k) => k.identity === choice.identity);
      if (x) x.votes = (x.votes ?? 0) + by;
    }
  };
  if (from) add(from, -weight);
  add(to, weight);
}

export function nameHandlers({
  wallet,
  grant,
  useGrant,
}: {
  wallet: (w: World, walletId: string) => FixtureWallet;
  grant: (w: World, id: string, walletId: string, kinds: ("spend" | "governance")[]) => FixtureGrant | null;
  useGrant: (w: World, id: string) => void;
}): Handlers {
  return {
    governance_contested_names: ({ walletId }, { world }) => {
      wallet(world, walletId);
      const n = world.governance.names;
      readable(n);
      return structuredClone(n.names);
    },
    governance_vote_name: ({ walletId, label, choice, masternodes, grant: grantId }, ctx: Ctx) => {
      const { world } = ctx;
      const n = world.governance.names;
      if (!LABEL.test(label) || (choice.kind === "towardsIdentity" && !BASE58.test(choice.identity)))
        throw fail("invalid_argument", "a label that cannot be contested, or a malformed identity");
      if (masternodes.length === 0 || new Set(masternodes).size !== masternodes.length || masternodes.some((m) => !HEX64.test(m)))
        throw fail("invalid_argument", "no masternodes, or a malformed or repeated proTxHash");
      readable(n);
      const contest = n.names.contests.find((c) => c.label === label);
      if (!contest)
        throw fail("governance.contest_not_found", `no contest for ${label}`, {
          kind: "contestLabel",
          label,
        });
      if (NOW >= contest.endsAt) throw fail("governance.contest_closed", `the contest for ${label} is closed`, { kind: "contestLabel", label });
      if (choice.kind === "towardsIdentity" && !contest.contenders.some((c) => c.identity === choice.identity))
        throw fail("governance.not_a_contender", `${choice.identity} is not a contender`, { kind: "contender", identity: choice.identity });
      wallet(world, walletId);
      if (!grant(world, grantId, walletId, ["governance"])) throw fail("platform.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      const allowed = n.names.votesAllowed;
      const casts: NameVoteCast[] = masternodes.map((proTxHash) => {
        const transition = transitionHash(`${proTxHash}:${label}:${JSON.stringify(choice)}`);
        const voter = n.names.voters.find((v) => v.proTxHash === proTxHash);
        if (!voter)
          return {
            proTxHash,
            transition: null,
            rejected: "unknown_masternode",
          };
        let state = voter.current.find((s) => s.label === label);
        const same = sameChoice(state?.choice, choice);
        // The engine's local limit (MG-07 D6, D18): nothing is sent.
        if ((state?.votesUsed ?? 0) >= allowed)
          return {
            proTxHash,
            transition: null,
            rejected: same ? "already_voted" : "vote_limit",
          };
        // Drive's 40304 for the same choice again, after the broadcast.
        if (same) return { proTxHash, transition, rejected: "already_voted" };
        const refused = n.refuse[proTxHash];
        if (refused && !(refused === "unconfirmed" && n.unconfirmedLands))
          return {
            proTxHash,
            transition: refused === "platform_unavailable" ? null : transition,
            rejected: refused,
          };
        if (!state) voter.current.push((state = { label, choice: null, votesUsed: 0 }));
        move(contest, state.choice, choice, voter.weight);
        state.choice = structuredClone(choice);
        // An unproved vote that did land is counted by Platform, not by this app (MG-07 D7).
        if (refused === "unconfirmed") return { proTxHash, transition, rejected: "unconfirmed" };
        state.votesUsed += 1;
        return { proTxHash, transition, rejected: null };
      });
      return casts;
    },
  };
}

// ---------------------------------------------------------------- scenarios

const A11CE = "a11ce";
const contest = (n: NamesWorld, label: string) => n.names.contests.find((c) => c.label === label)!;

/** `base` builds the governance base world with `names` patched in. */
export function nameScenarios(base: (theme: ThemeName | null, patch?: (n: NamesWorld) => void) => World) {
  return {
    /** MG-07's fixture as it is: two contests, three voters (one evonode at its vote limit on a11ce, one
     * banned masternode with an attached key). */
    names: (t: ThemeName | null) => base(t),
    /** No open contest. */
    "names-empty": (t: ThemeName | null) => base(t, (n) => (n.names.contests = [])),
    /** No masternode whose voting key the wallet holds: browsing only. */
    "names-no-voters": (t: ThemeName | null) => base(t, (n) => (n.names.voters = [])),
    /** SPV has no masternode list yet. */
    "names-list-unavailable": (t: ThemeName | null) => base(t, (n) => (n.unavailable = "list")),
    /** Platform can't be reached. */
    "names-platform-unavailable": (t: ThemeName | null) => base(t, (n) => (n.unavailable = "platform")),
    /** a11ce's voting ends in three hours. */
    "names-ending": (t: ThemeName | null) => base(t, (n) => (contest(n, A11CE).endsAt = NOW + 3 * 3600)),
    /** a11ce's voting ended ten minutes ago; Platform hasn't resolved it yet, so it is still listed. */
    "names-ended": (t: ThemeName | null) => base(t, (n) => (contest(n, A11CE).endsAt = NOW - 600)),
    /** a11ce has a full first page of contenders: later ones may exist (MG-07 D9). */
    "names-partial": (t: ThemeName | null) => base(t, (n) => (contest(n, A11CE).contendersComplete = false)),
    /** The regular masternode's vote goes out with no answer (it did land); the banned one isn't in
     * Platform's list yet. */
    "names-vote-mixed": (t: ThemeName | null) =>
      base(t, (n) => {
        n.refuse = {
          [VOTER_A]: "unconfirmed",
          [VOTER_C]: "masternode_not_found",
        };
        n.unconfirmedLands = true;
      }),
    /** A watch-only wallet browses. */
    "names-watch-only": (t: ThemeName | null) => {
      const w = base(t);
      w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
      return w;
    },
  };
}
