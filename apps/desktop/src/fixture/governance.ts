// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's governance (MG-09 over mg-engine §2): a synced store of proposals with tallies, the
// main wallet's voting masternodes and the votes they cast, and the wallet's own proposals in
// MG-06's states. The commands refuse as the facade does (`not_synced` before a run reached
// `synced`, `invalid_argument` for a malformed hash, `submit_refused` past submitting, the
// grant's purpose and cap). The numbers are mainnet-shaped, as the vectors are (the 16,616-block
// cycle, so the clock reads in days); a scenario or a test changes the world and `emit`s
// `governance` as the engine's notifier would.

import type { AppError } from "../api/generated/commands";
import { BANNED_REJECTION } from "../governance/model";
import type { GovernanceInfo } from "../api/generated/GovernanceInfo";
import type { GovernanceSyncState } from "../api/generated/GovernanceSyncState";
import type { MyProposal } from "../api/generated/MyProposal";
import type { MyProposalState } from "../api/generated/MyProposalState";
import type { ProposalDetail } from "../api/generated/ProposalDetail";
import type { ProposalDraft } from "../api/generated/ProposalDraft";
import type { ProposalFieldName } from "../api/generated/ProposalFieldName";
import type { ProposalRow } from "../api/generated/ProposalRow";
import type { ProposalStatusName } from "../api/generated/ProposalStatusName";
import type { SuperblockDate } from "../api/generated/SuperblockDate";
import type { ThemeName } from "../api/generated/ThemeName";
import type { VoteCast } from "../api/generated/VoteCast";
import type { VoteOutcomeName } from "../api/generated/VoteOutcomeName";
import type { VoteTally } from "../api/generated/VoteTally";
import type { VotingMasternode } from "../api/generated/VotingMasternode";
import type { WeightRange } from "../api/generated/WeightRange";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

const COIN = 100_000_000;
const NOW = secs(FIXTURE_NOW);
/** The engine's ETA spacing (MG-02 D9). */
const SPACING = 157.2;
const CYCLE = 16_616;
const MATURITY = 1_662;
const TIP = 2_372_666;
const NEXT = 2_375_048;
/** `proposal_fee` (MG-06 D16) and the fee transaction's network fee. */
export const PROPOSAL_FEE = COIN;
export const FEE_NETWORK = 227;
const BUDGET = 6_018 * COIN;
const THRESHOLD = 357;
const HEX64 = /^[0-9a-f]{64}$/;

const eta = (height: number) => Math.round(NOW + (height - TIP) * SPACING);
const h = (c: string) => c.repeat(Math.ceil(64 / c.length)).slice(0, 64);

interface FixtureVote {
  outcome: VoteOutcomeName;
  time: number;
}

export interface GovernanceWorld {
  enabled: boolean;
  sync: GovernanceSyncState;
  /** The phase `set_sync_enabled(true)` and `resync` go to. */
  phaseOnStart: GovernanceSyncState["phase"];
  /** Every confirmed proposal, in the engine's order (status groups, then net yes). */
  proposals: ProposalDetail[];
  /** The tip list's masternodes whose voting key the main wallet derives (a watch-only wallet's
   * xpub-derived keys are the same). */
  masternodes: Omit<VotingMasternode, "currentVote" | "voteTime" | "keyLastVote" | "keyLastVoteTime">[];
  /** Votes seen, by proposal then ProTx hash. */
  votes: Record<string, Record<string, FixtureVote>>;
  /** `vote` refuses these masternodes with the reason (a store refusal, mg-engine §2.4). */
  refuse: Record<string, string>;
  /** `governance.list_unavailable` while SPV has no masternode list. */
  listUnavailable: boolean;
  /** By wallet id, newest first. */
  mine: Record<string, MyProposal[]>;
  /** A [Submit] whose resend fails, or whose new run finds no peer again. */
  submitFails: boolean;
}

const range = (estimate: number, slack = 0): WeightRange => ({ min: Math.max(0, estimate - slack), max: estimate + slack, estimate });

function tally(yes: number, no: number, abstain: number, slack = 0): VoteTally {
  return {
    yes: range(yes, slack),
    no: range(no),
    abstain: range(abstain),
    netYesEstimate: yes - no,
    voters: Math.round((yes + no + abstain) * 0.8),
    rangedVotes: slack,
  };
}

function proposal(c: string, name: string, amount: number, payments: number, status: ProposalStatusName, t: VoteTally, closeCall = false, ageDays = 10): ProposalDetail {
  const start = NOW - ageDays * 86_400;
  const url = `https://www.dashcentral.org/p/${name}`;
  const paymentAddress = `yQ${c.repeat(4).slice(0, 4)}Gov8vK2pMxRt5nW9cJ3hL6dF4sA1bZ7Wm4c`.slice(0, 34);
  const row: ProposalRow = {
    hash: h(c),
    name,
    url,
    paymentAddress,
    paymentAmount: amount * COIN,
    startEpoch: start,
    endEpoch: start + payments * CYCLE * 150 + 86_400 * 14,
    paymentsRequested: payments,
    status,
    closeCall,
    tally: t,
    fundedHeight: status === "funded" ? NEXT - CYCLE : null,
  };
  return {
    row,
    collateralTxid: h(c === "f" ? "e" : "f"),
    createdAt: start,
    dataJson: JSON.stringify({ end_epoch: row.endEpoch, name, payment_address: paymentAddress, payment_amount: amount, start_epoch: start, type: 1, url }),
    signals: { valid: tally(0, 0, 0), delete: tally(0, 0, 0), endorsed: tally(0, 0, 0) },
    deletionMarkedAt: null,
  };
}

/** MG-00's `gov-list` sample, on testnet addresses. */
export const FIXTURE_PROPOSALS: ProposalDetail[] = [
  proposal("1", "dash-incubator-nov", 1500, 3, "passing", tally(812, 41, 9, 12), false, 12),
  proposal("2", "translations-2027", 220, 1, "unfunded", tally(492, 41, 5), false, 20),
  proposal("3", "alt-wallet-hardening", 800, 6, "voting", tally(380, 29, 9, 12), true, 3),
  proposal("4", "dashcon-2027-venue", 3000, 1, "voting", tally(410, 745, 100), false, 25),
  proposal("5", "node-docs-refresh", 90, 2, "failing", tally(60, 300, 10), false, 7),
  proposal("6", "old-website-redesign", 400, 2, "lapsed", tally(900, 20, 3), false, 90),
];

export const MN_A = h("a1");
export const MN_B = h("b2");
export const MN_C = h("c3");

const FIXTURE_MASTERNODES: GovernanceWorld["masternodes"] = [
  { proTxHash: MN_A, kind: "regular", weight: 1, valid: true, keyIndex: 0, keyHolders: 1 },
  { proTxHash: MN_B, kind: "evonode", weight: 4, valid: true, keyIndex: 1, keyHolders: 1 },
  { proTxHash: MN_C, kind: "regular", weight: 1, valid: false, keyIndex: 2, keyHolders: 1 },
];

const synced = (): GovernanceSyncState => ({
  phase: "synced",
  objects: FIXTURE_PROPOSALS.length + 2,
  unconfirmedObjects: 0,
  votes: 41_250,
  orphanVotes: 0,
  peers: 8,
  bytesReceived: 9_876_543,
  lastSyncedAt: NOW - 120,
  lastError: null,
  nextRetryAt: null,
  objectsSecs: 1.5,
  votesSecs: 12.25,
});

const disabled = (): GovernanceSyncState => ({ ...synced(), phase: "disabled", objects: 0, votes: 0, peers: 0, bytesReceived: 0, lastSyncedAt: null, objectsSecs: null, votesSecs: null });

export const newGovernanceWorld = (): GovernanceWorld => ({
  enabled: false,
  sync: disabled(),
  phaseOnStart: "synced",
  proposals: structuredClone(FIXTURE_PROPOSALS),
  masternodes: structuredClone(FIXTURE_MASTERNODES),
  votes: { [h("1")]: { [MN_A]: { outcome: "yes", time: NOW - 40 * 60 } }, [h("2")]: { [MN_A]: { outcome: "yes", time: NOW - 3 * 86_400 }, [MN_B]: { outcome: "yes", time: NOW - 3 * 86_400 } } },
  refuse: {},
  listUnavailable: false,
  mine: {},
  submitFails: false,
});

/** A synced governance world with the sample proposals (the base of every governance scenario). */
export function syncedGovernance(): GovernanceWorld {
  return { ...newGovernanceWorld(), enabled: true, sync: synced() };
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

function requireSynced(w: GovernanceWorld) {
  if (w.sync.phase !== "synced") throw fail("governance.not_synced", `not synced: ${w.sync.phase}`, { kind: "governancePhase", phase: w.sync.phase });
}

function find(w: GovernanceWorld, hash: string): ProposalDetail {
  requireSynced(w);
  if (!HEX64.test(hash)) throw fail("invalid_argument", "not a 64-character hex hash");
  const p = w.proposals.find((x) => x.row.hash === hash);
  if (!p) throw fail("governance.proposal_not_found", `proposal not found: ${hash}`, { kind: "proposalHash", hash });
  return p;
}

function upcoming(count: number): SuperblockDate[] {
  return Array.from({ length: count }, (_, i) => {
    const height = NEXT + i * CYCLE;
    const cutoff = height - MATURITY;
    return {
      height,
      estimatedTime: eta(height),
      budget: BUDGET,
      votingCutoffHeight: cutoff,
      votingCutoffEstimate: eta(cutoff),
      cutoffSoon: eta(cutoff) - NOW <= 172_800,
    };
  });
}

/** `info` as the engine builds it: the clock and the list's numbers; the counts once synced. */
export function governanceInfo(w: GovernanceWorld): GovernanceInfo {
  const cutoff = NEXT - MATURITY;
  const rows = w.proposals.map((p) => p.row).filter((r) => r.status !== "lapsed");
  const count = (...s: ProposalStatusName[]) => rows.filter((r) => s.includes(r.status)).length;
  const ready = w.sync.phase === "synced";
  const allocated = rows.filter((r) => r.status === "passing" || r.status === "funded").reduce((s, r) => s + r.paymentAmount, 0);
  return {
    clock: {
      tipHeight: TIP,
      cycleProgress: (TIP - (NEXT - CYCLE)) / CYCLE,
      lastSuperblock: NEXT - CYCLE,
      nextSuperblock: NEXT,
      blocksToSuperblock: NEXT - TIP,
      superblockEta: eta(NEXT),
      votingCutoffEstimate: eta(cutoff),
      votingCutoffHeight: cutoff,
      votingOpen: TIP < cutoff,
    },
    superblockCycle: CYCLE,
    maturityWindow: MATURITY,
    targetSpacing: 150,
    proposalFee: PROPOSAL_FEE,
    budget: BUDGET,
    passingThreshold: THRESHOLD,
    validMasternodes: 3_030,
    validEvonodes: 180,
    validWeight: 3_570,
    proposals: ready
      ? { passing: count("passing", "funded"), failing: count("failing", "voting"), unfunded: count("unfunded"), total: rows.length, closeCall: rows.filter((r) => r.closeCall).length }
      : null,
    allocatedEstimate: ready ? allocated : null,
    unallocatedEstimate: ready ? BUDGET - allocated : null,
  };
}

function votingFor(w: GovernanceWorld, proposal: string): VotingMasternode[] {
  const seen = w.votes[proposal] ?? {};
  return w.masternodes.map((m) => ({
    ...m,
    currentVote: seen[m.proTxHash]?.outcome ?? null,
    voteTime: seen[m.proTxHash]?.time ?? null,
    keyLastVote: null,
    keyLastVoteTime: null,
  }));
}

/** The draft's failing fields in wizard order (MG-06 D11), as `preview_proposal` checks them. */
function draftProblems(draft: ProposalDraft): ProposalFieldName[] {
  const out: ProposalFieldName[] = [];
  if (!/^[a-zA-Z0-9_-]{1,40}$/.test(draft.name)) out.push("name");
  if (!/^https?:\/\/\S+$/.test(draft.url) || draft.url.length > 255) out.push("url");
  if (!/^y[1-9A-HJ-NP-Za-km-z]{33}$/.test(draft.paymentAddress)) out.push("payment_address");
  if (!(draft.paymentAmount > 0) || draft.paymentAmount > BUDGET) out.push("payment_amount");
  if (!(draft.paymentCount >= 1 && draft.paymentCount <= 12)) out.push("payment_count");
  if (!upcoming(12).some((s) => s.height === draft.firstSuperblockHeight)) out.push("first_payment");
  return out;
}

function checkDraft(draft: ProposalDraft) {
  const fields = draftProblems(draft);
  if (fields.length > 0) throw fail("governance.invalid_proposal", `invalid proposal: ${fields.join(", ")}`, { kind: "proposalFields", fields });
}

/** A stable 64-hex "hash" of the draft (FNV-1a over its JSON, repeated). */
function draftHash(draft: ProposalDraft): string {
  let x = 0x811c9dc5;
  for (const ch of JSON.stringify(draft)) x = Math.imul(x ^ ch.charCodeAt(0), 0x01000193) >>> 0;
  return x.toString(16).padStart(8, "0").repeat(8);
}

function epochs(draft: ProposalDraft) {
  const start = eta(draft.firstSuperblockHeight) - (CYCLE / 2) * 150;
  return { start, end: start + draft.paymentCount * CYCLE * 150 };
}

function mineOf(world: World, walletId: string): MyProposal[] {
  return (world.governance.mine[walletId] ??= []);
}

/** What a handler returns is a copy, as IPC's would be: the world's objects change in place. */
const copy = <T>(v: T): T => structuredClone(v);

const changed = ({ world, emit }: Ctx) => emit({ kind: "governance", network: world.launch.network.dirName });

export function governanceHandlers({
  wallet,
  grant,
  useGrant,
}: {
  wallet: (w: World, walletId: string) => FixtureWallet;
  grant: (w: World, id: string, walletId: string, kinds: ("spend" | "governance")[]) => FixtureGrant | null;
  useGrant: (w: World, id: string) => void;
}): Handlers {
  return {
    governance_sync_enabled: (_, { world }) => world.governance.enabled,
    governance_set_sync_enabled: ({ enabled }, ctx) => {
      const g = ctx.world.governance;
      if (g.enabled !== enabled) {
        g.enabled = enabled;
        g.sync = enabled ? { ...synced(), phase: g.phaseOnStart } : disabled();
        changed(ctx);
      }
      return null;
    },
    governance_sync_state: (_, { world }) => copy(world.governance.sync),
    governance_resync: (_, ctx) => {
      const g = ctx.world.governance;
      if (!g.enabled) throw fail("governance.sync_disabled", "governance sync is disabled");
      g.sync = { ...g.sync, phase: g.phaseOnStart, lastError: null, nextRetryAt: null };
      changed(ctx);
      return null;
    },
    governance_clock: (_, { world }) => governanceInfo(world.governance).clock,
    governance_info: (_, { world }) => governanceInfo(world.governance),
    governance_upcoming_superblocks: ({ count }) => {
      if (count > 24) throw fail("invalid_argument", "count above 24");
      return upcoming(count);
    },
    governance_proposals: ({ query }, { world }) => {
      const g = world.governance;
      requireSynced(g);
      const text = query.text?.toLowerCase() ?? "";
      return g.proposals
        .map((p) => copy(p.row))
        .filter((r) => query.scope === "all" || r.status !== "lapsed")
        .filter((r) => text === "" || r.name.toLowerCase().includes(text) || r.url.toLowerCase().includes(text));
    },
    governance_proposal: ({ hash }, { world }) => copy(find(world.governance, hash)),
    governance_current_votes: ({ hash }, { world }) => {
      find(world.governance, hash);
      return [];
    },
    governance_voting_masternodes: ({ walletId, proposal }, { world }) => {
      wallet(world, walletId);
      if (!HEX64.test(proposal)) throw fail("invalid_argument", "not a 64-character hex hash");
      if (world.governance.listUnavailable) throw fail("governance.list_unavailable", "masternode list unavailable");
      return votingFor(world.governance, proposal);
    },
    governance_vote: ({ walletId, proposal, outcome, masternodes, grant: grantId }, ctx) => {
      const { world } = ctx;
      const g = world.governance;
      wallet(world, walletId);
      if (!g.enabled) throw fail("governance.sync_disabled", "governance sync is disabled");
      if (g.listUnavailable) throw fail("governance.list_unavailable", "masternode list unavailable");
      const p = find(g, proposal);
      if (masternodes.length === 0 || new Set(masternodes).size !== masternodes.length || masternodes.some((m) => !HEX64.test(m)))
        throw fail("invalid_argument", "no masternodes, or a malformed or repeated proTxHash");
      if (!grant(world, grantId, walletId, ["governance"])) throw fail("platform.grant_invalid", "grant invalid");
      useGrant(world, grantId);
      const seen = (g.votes[proposal] ??= {});
      const casts: VoteCast[] = masternodes.map((proTxHash) => {
        const m = g.masternodes.find((x) => x.proTxHash === proTxHash);
        const refused = (rejected: string): VoteCast => ({ proTxHash, hash: null, peers: 0, rejected });
        if (!m) return refused("unknown_masternode");
        if (g.refuse[proTxHash]) return refused(g.refuse[proTxHash]);
        if (!m.valid) return refused(BANNED_REJECTION);
        const before = seen[proTxHash];
        if (before && NOW - before.time < 3600) return refused("too_often");
        seen[proTxHash] = { outcome, time: NOW };
        // The tally follows (estimates only; the ranges stay as they were).
        const t = p.row.tally;
        if (before) t[before.outcome] = range(t[before.outcome].estimate - m.weight);
        t[outcome] = range(t[outcome].estimate + m.weight);
        t.netYesEstimate = t.yes.estimate - t.no.estimate;
        return { proTxHash, hash: draftHash({ name: proTxHash, url: outcome } as ProposalDraft), peers: g.sync.peers, rejected: null };
      });
      changed(ctx);
      return casts;
    },
    governance_preview_proposal: ({ walletId, draft }, { world }) => {
      wallet(world, walletId);
      checkDraft(draft);
      const { start, end } = epochs(draft);
      const dataJson = JSON.stringify({ end_epoch: end, name: draft.name, payment_address: draft.paymentAddress, payment_amount: draft.paymentAmount / COIN, start_epoch: start, type: 1, url: draft.url });
      const dataHex = Array.from(new TextEncoder().encode(dataJson), (b) => b.toString(16).padStart(2, "0")).join("");
      return { hash: draftHash(draft), dataJson, dataHex, startEpoch: start, endEpoch: end, fee: PROPOSAL_FEE, networkFee: FEE_NETWORK };
    },
    governance_create_proposal: ({ walletId, draft, grant: grantId }, ctx) => {
      const { world } = ctx;
      const w = wallet(world, walletId);
      checkDraft(draft);
      const hash = draftHash(draft);
      const mine = mineOf(world, walletId);
      if (mine.some((p) => p.hash === hash)) throw fail("governance.submit_refused", "submit refused: in_flight", { kind: "reason", reason: "in_flight" });
      const g = grant(world, grantId, walletId, ["spend"]);
      if (!g || g.grant.purpose.kind !== "spend") throw fail("send.grant_invalid", "grant invalid");
      const cost = PROPOSAL_FEE + FEE_NETWORK;
      if (g.grant.purpose.maxDuffs < cost) throw fail("send.grant_exceeded", "grant exceeded", { kind: "spendCap", maxDuffs: g.grant.purpose.maxDuffs, outflow: cost });
      const available = w.summary.balances?.confirmed ?? 0;
      if (available < cost) throw fail("send.amount_with_fee_exceeds_balance", "amount with fee exceeds balance", { kind: "duffs", needed: cost, available });
      useGrant(world, grantId);
      const { start, end } = epochs(draft);
      const created: MyProposal = {
        hash,
        name: draft.name,
        url: draft.url,
        paymentAddress: draft.paymentAddress,
        paymentAmount: draft.paymentAmount,
        paymentCount: draft.paymentCount,
        startEpoch: start,
        endEpoch: end,
        feeTxid: hash.split("").reverse().join(""),
        createdAt: NOW,
        submittedAt: null,
        state: { kind: "feePaid", confirmations: 0 },
        lastError: null,
      };
      mine.unshift(created);
      changed(ctx);
      return copy(created);
    },
    governance_my_proposals: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return copy(mineOf(world, walletId));
    },
    governance_submit_proposal: ({ walletId, hash }, ctx) => {
      const { world } = ctx;
      const g = world.governance;
      wallet(world, walletId);
      if (!HEX64.test(hash)) throw fail("invalid_argument", "not a 64-character hex hash");
      const mine = mineOf(world, walletId);
      const i = mine.findIndex((p) => p.hash === hash);
      if (i < 0) throw fail("governance.proposal_not_found", `proposal not found: ${hash}`, { kind: "proposalHash", hash });
      const p = mine[i];
      const refuse = (reason: string) => fail("governance.submit_refused", `submit refused: ${reason}`, { kind: "reason", reason });
      const set = (state: MyProposalState, patch: Partial<MyProposal> = {}) => {
        mine[i] = { ...p, ...patch, state };
        changed(ctx);
        return copy(mine[i]);
      };
      const s = p.state;
      if (s.kind === "public") throw refuse("public");
      if (s.kind === "expired") throw refuse("expired");
      if (s.kind === "feeFailed") throw refuse("fee_failed");
      // A resend of the saved fee (MG-06 D10): no grant, works with sync off.
      if (s.kind === "feeUnsent" || (s.kind === "feePaid" && s.confirmations === 0)) {
        if (g.submitFails) {
          const lastError = "send: broadcast outcome unknown: no peer acknowledged the transaction";
          mine[i] = { ...p, state: { kind: "feeUnsent" }, lastError };
          changed(ctx);
          throw fail("send.broadcast_unknown", lastError);
        }
        return set({ kind: "feePaid", confirmations: 0 }, { lastError: null });
      }
      if (!g.enabled) throw fail("governance.sync_disabled", "governance sync is disabled");
      if (s.kind === "readyToSubmit" && s.reason === "noPeerAccepted" && !g.submitFails) return set({ kind: "confirming", confirmations: 1 }, { submittedAt: NOW });
      return copy(p);
    },
  };
}

// ---------------------------------------------------------------- scenarios

const mineRow = (n: number, name: string, state: MyProposalState, patch: Partial<MyProposal> = {}): MyProposal => ({
  hash: h(`d${n}`),
  name,
  url: `https://www.dashcentral.org/p/${name}`,
  paymentAddress: "yPay8vK2pMxRt5nW9cJ3hL6dF4sA1bZ7Wm",
  paymentAmount: 120 * COIN,
  paymentCount: 3,
  startEpoch: NOW + 86_400,
  endEpoch: NOW + 90 * 86_400,
  feeTxid: h(`e${n}`),
  createdAt: NOW - n * 3600,
  submittedAt: state.kind === "confirming" || state.kind === "public" ? NOW - n * 3000 : null,
  state,
  lastError: null,
  ...patch,
});

/** One own proposal in each MG-06 state (MG-09 fixture scenarios). */
const EVERY_STATE: MyProposal[] = [
  mineRow(1, "desktop-wallet-i18n", { kind: "feePaid", confirmations: 0 }),
  mineRow(2, "dash-docs-refresh", { kind: "confirming", confirmations: 4 }),
  mineRow(3, "community-meetups", { kind: "readyToSubmit", reason: "noPeerAccepted" }),
  mineRow(4, "wallet-audit-2027", { kind: "feeUnsent" }, { lastError: "send: broadcast outcome unknown: the resend did not finish" }),
  mineRow(5, "school-outreach", { kind: "readyToSubmit", reason: "notSynced" }),
  mineRow(6, "merchant-kit", { kind: "feePaid", confirmations: 1 }),
  mineRow(7, "dash-incubator-nov", { kind: "public", status: "passing" }, { hash: h("1") }),
  mineRow(8, "podcast-season-3", { kind: "feeFailed" }),
  mineRow(9, "old-hackathon", { kind: "expired" }),
];

type Base = (theme: ThemeName | null) => World;

/** Governance with the sample proposals and the main wallet's masternodes, the item shown. */
function governanceBase(base: Base, theme: ThemeName | null, patch: Partial<GovernanceWorld> = {}): World {
  const w = base(theme);
  w.governance = { ...syncedGovernance(), ...patch };
  w.settings.ui = { ...w.settings.ui, showMasternodes: true };
  return w;
}

export function governanceScenarios(base: Base) {
  const id = (w: World) => w.wallets[0].summary.walletId;
  return {
    /** The `gov-list` mock: synced, three voting masternodes (one banned), one own proposal confirming. */
    governance: (t: ThemeName | null) => {
      const w = governanceBase(base, t);
      w.governance.mine[id(w)] = [EVERY_STATE[1]];
      return w;
    },
    /** The `gov-mine` mock: an own proposal in every MG-06 state. */
    "governance-mine": (t: ThemeName | null) => {
      const w = governanceBase(base, t);
      w.governance.mine[id(w)] = structuredClone(EVERY_STATE);
      return w;
    },
    /** `gov-syncing`: votes loading, no counts yet (MG-00 D21); Mine shows below (D32). */
    "governance-syncing": (t: ThemeName | null) => {
      const w = governanceBase(base, t, { sync: { ...synced(), phase: "syncingVotes", votes: 1_204, lastSyncedAt: null }, phaseOnStart: "syncingVotes" });
      w.governance.mine[id(w)] = [EVERY_STATE[1]];
      return w;
    },
    /** Sync off (D6): the Disabled state with Turn on. */
    "governance-disabled": (t: ThemeName | null) => governanceBase(base, t, { enabled: false, sync: disabled() }),
    /** No peer serves governance data. */
    "governance-failed": (t: ThemeName | null) => {
      const w = governanceBase(base, t, { sync: { ...synced(), phase: "failed", lastError: "no peer completed its sync", nextRetryAt: NOW + 30 }, phaseOnStart: "failed" });
      w.governance.mine[id(w)] = [EVERY_STATE[1]];
      return w;
    },
    /** No masternode whose voting key the wallet holds: browsing only. */
    "governance-no-masternodes": (t: ThemeName | null) => governanceBase(base, t, { masternodes: [] }),
    /** Masternodes whose voting keys are here but none can vote (banned). */
    "governance-banned": (t: ThemeName | null) => governanceBase(base, t, { masternodes: FIXTURE_MASTERNODES.map((m) => ({ ...m, valid: false })) }),
    /** The vote is refused for the evonode: its voting key changed. */
    "governance-vote-refused": (t: ThemeName | null) => governanceBase(base, t, { refuse: { [MN_B]: "wrong_voting_key" } }),
    /** A [Submit] fails: the resend's broadcast is unknown, a new run finds no peer again. */
    "governance-submit-fails": (t: ThemeName | null) => {
      const w = governanceBase(base, t, { submitFails: true });
      w.governance.mine[id(w)] = structuredClone(EVERY_STATE);
      return w;
    },
    /** No proposal this cycle. */
    "governance-empty": (t: ThemeName | null) => governanceBase(base, t, { proposals: [] }),
    /** A watch-only wallet browses (§4.25.1). */
    "governance-watch-only": (t: ThemeName | null) => {
      const w = governanceBase(base, t);
      w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
      return w;
    },
  };
}
