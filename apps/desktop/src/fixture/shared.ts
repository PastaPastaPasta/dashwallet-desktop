// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's shared masternodes (MG-10 §5.1, MG-10b S5, over mn-engine §2.4): the capability, the
// main wallet's sessions and the calls that move them, refusing in the engine's order. Messages are
// fixed fake JSON blobs (no keys, documentation IPs, `y…` addresses); an import looks its text up in
// `SH_TEXT`, so a spec pastes what another party "sent". A change `emit`s `masternodes`.

import type { AppError } from "../api/generated/commands";
import type { SharedCapability } from "../api/generated/SharedCapability";
import type { SharedImport } from "../api/generated/SharedImport";
import type { SharedMessage } from "../api/generated/SharedMessage";
import type { SharedSession } from "../api/generated/SharedSession";
import type { ShareRow } from "../api/generated/ShareRow";
import type { ShareTermsView } from "../api/generated/ShareTermsView";
import type { StandbyDescription } from "../api/generated/StandbyDescription";
import type { UnreadableSession } from "../api/generated/UnreadableSession";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureWallet, World } from "./scenarios";
import { onMainnet } from "./rates";
import { FIXTURE_NOW, secs } from "./wallet";

const COIN = 100_000_000;
const NOW = secs(FIXTURE_NOW);
const DAY = 86_400;
const FEE = 100_000;
const SHARE_TOTAL = 1_000 * COIN;
const HEX64 = /^[0-9a-f]{64}$/;
/** The largest message the engine reads (MG-04b §2.2). */
const MAX_TEXT = 2 << 20;

const h = (c: string) => c.repeat(Math.ceil(64 / c.length)).slice(0, 64);
const fake = (tag: string, len: number) => tag.repeat(Math.ceil(len / tag.length)).slice(0, len);
const addr = (tag: string) => `y${tag.replace(/[0OIl]/g, "z")}${fake("Sh4rEdMn7qR2vWx9K", 33 - tag.length)}`;

/** The coordinator session this wallet runs ("you coordinate"), and the one it joined. */
export const SH_COORD = `3f9a2c${h("a1").slice(6)}`;
export const SH_PART = `7b21e0${h("b2").slice(6)}`;
export const SH_SERVICE = "203.0.113.30:19999";
const OPERATOR = fake("93ac5e1f0", 96);
const VOTING = addr("vS");

export interface SharedWorld {
  capability: SharedCapability;
  /** The main wallet's sessions. */
  sessions: SharedSession[];
  /** The next file `mn_shared_import_file` "opens": the texts it holds, an error code, or `null` (cancel). */
  nextFile: { texts: string[] } | { error: "app.file_read" | "app.file_too_large" | "fileNotMessage" } | null;
  /** Message texts the main wallet copied, newest last, for specs to check. */
  saved: string[];
  /** Standbys this wallet holds, by txid. */
  standbys: string[];
  /** Sessions whose stored record no longer reads. */
  unreadable: UnreadableSession[];
  /** Sent sessions this wallet abandoned: no Send again (D72). */
  abandoned: string[];
  /** The `last_error` the next registration hand-off leaves: `null` when it went out. */
  handoff: string | null;
  /** How the next abandon spend's hand-off ends: `null` when it went out and the wallet recorded it. */
  abandonHandoff: "uncertain" | "refused" | null;
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });
const copy = <T>(v: T): T => structuredClone(v);
const refuse = (reason: string, share: number | null = null) => fail("masternodes.shared_message", `shared masternode message refused: ${reason}`, { kind: "sharedMessage", reason, share });
/** The stage `abandon_shared_session` names when the session is out and no reserved coin is left to spend. */
const outStage = (role: SharedSession["role"]) => (role === "coordinator" ? "sent" : "funding_signed");
const stageRefusal = (stage: string) => fail("masternodes.shared_stage", `the shared session is ${stage}`, { kind: "sharedStage", stage });
const changed = ({ world, emit }: Ctx) => emit({ kind: "masternodes", network: world.launch.network.dirName });

const terms = (patch: Partial<ShareTermsView> = {}): ShareTermsView => ({
  service: SH_SERVICE,
  serviceRead: null,
  operatorPublicKey: OPERATOR,
  votingAddress: VOTING,
  operatorReward: 500,
  earlyPeriodBlocks: 105_120,
  earlyPenalty: 20 * COIN,
  ...patch,
});

function shareRow(index: number, label: string, dash: number, patch: Partial<ShareRow> = {}): ShareRow {
  return {
    index,
    label,
    amount: dash * COIN,
    ownerAddress: addr(`o${label.slice(0, 2)}`),
    refundAddress: addr(`r${label.slice(0, 2)}`),
    rewardAddress: null,
    mine: false,
    contributed: true,
    approved: false,
    fundingSigned: false,
    ...patch,
  };
}

const code = (id: string) => id.slice(0, 6).toUpperCase();
const messageCode = (tag: string) => {
  const s = fake(tag.toUpperCase().replace(/[^0-9A-F]/g, "") || "AB", 8);
  return `${s.slice(0, 4)}-${s.slice(4)}`;
};

/** The relay minimum of an abandon spend of `inputs` P2PKH coins to one P2PKH output, as the
 * engine's coin test charges it. */
export const abandonFee = (inputs: number): number => 45 + 148 * inputs;

/** The recovery read's Abandon charge: every reserved coin spent, while Abandon is offered. */
const withCharge = <T extends { abandonable: boolean; reserved: string[]; abandonCharge: number | null }>(s: T): T => ({ ...s, abandonCharge: s.abandonable ? abandonFee(s.reserved.length) : null });

function session(id: string, role: SharedSession["role"], patch: Partial<SharedSession>): SharedSession {
  return withCharge({
    id,
    code: code(id),
    messageCode: messageCode(`${id.slice(6, 10)}${patch.revision ?? 1}`),
    kind: "registration",
    role,
    stage: { kind: "draft" },
    revision: 1,
    consentHash: null,
    rows: [],
    terms: terms(),
    nodeRunBy: "Alice",
    fee: role === "coordinator" ? FEE : null,
    debit: null,
    proTxHash: null,
    txid: null,
    reserved: [],
    next: "copy",
    discardable: true,
    abandonable: false,
    warnings: [],
    approvedTerms: null,
    baseKeys: null,
    keyChange: null,
    actorFee: null,
    createdAt: NOW - 2 * DAY,
    updatedAt: NOW - DAY,
    lastError: null,
    overdue: false,
    abandonCharge: null,
    abandonSpend: null,
    resendAbandonCharge: null,
    ...patch,
  });
}

/** Alice (this wallet) coordinates Alice 400 · Bob 300 · Carol 300. `joined` rows are in. */
export function coordinatorSession(joined: ("Bob" | "Carol")[] = [], patch: Partial<SharedSession> = {}): SharedSession {
  const rows = [
    shareRow(0, "Alice", 400, { mine: true }),
    shareRow(1, "Bob", 300, joined.includes("Bob") ? {} : { ownerAddress: null, refundAddress: null, contributed: false }),
    shareRow(2, "Carol", 300, joined.includes("Carol") ? {} : { ownerAddress: null, refundAddress: null, contributed: false }),
  ];
  const all = joined.length === 2;
  return session(SH_COORD, "coordinator", {
    rows,
    next: all ? "lock" : "copy",
    debit: 400 * COIN + FEE,
    reserved: [`${h("c0")}-0`],
    ...patch,
  });
}

/** Alice invited this wallet ("you") to share 3; Bob holds 300. */
export function participantSession(stage: SharedSession["stage"]["kind"], patch: Partial<SharedSession> = {}): SharedSession {
  const joined = stage !== "invited";
  const rows = [
    shareRow(0, "Alice", 400, { approved: stage !== "invited" && stage !== "draft" }),
    shareRow(1, "Bob", 300),
    shareRow(2, "Dana", 300, joined ? { mine: true } : { ownerAddress: null, refundAddress: null, contributed: false }),
  ];
  // From `combined` on, every party approved the locked terms (the engine's combine needs them all).
  const out = stage === "funding_signed" || stage === "sent" || stage === "confirmed" || stage === "conflicted";
  if (stage === "combined" || out) for (const r of rows) r.approved = true;
  const next: SharedSession["next"] = (
    {
      invited: "join",
      draft: "copy",
      signing: "approve",
      combined: "sign",
      funding_signed: "copy",
      sent: "wait",
      confirmed: "nothing",
      conflicted: "nothing",
      dead: "nothing",
      stale: "nothing",
    } as const
  )[stage];
  const settled = stage === "confirmed" || stage === "conflicted";
  const stageValue: SharedSession["stage"] =
    stage === "confirmed" ? { kind: "confirmed", height: 1_214_800 } : stage === "dead" ? { kind: "dead", reason: "inputs_spent" } : ({ kind: stage } as SharedSession["stage"]);
  return session(SH_PART, "participant", {
    rows,
    stage: stageValue,
    next,
    revision: stage === "invited" ? 1 : 3,
    consentHash: stage === "invited" || stage === "draft" ? null : h("cd"),
    debit: joined ? 300 * COIN : null,
    reserved: joined ? [`${h("d0")}-1`] : [],
    discardable: !out,
    abandonable: out && !settled && joined,
    fee: null,
    ...patch,
  });
}

function message(s: SharedSession, name: string): SharedMessage {
  const text = JSON.stringify({ type: "dash-shared-mn-session", version: 1, network: "test", sessionId: s.id, revision: s.revision, stage: s.stage.kind, fixture: name.toLowerCase().replace(/ /g, "-") }, null, 2);
  return { text, code: s.messageCode, name, fileName: `shared-mn-session-${s.code}.json` };
}

function messageName(s: SharedSession): string {
  const c = s.role === "coordinator";
  switch (s.stage.kind) {
    case "invited":
    case "draft":
      return c ? "Invitation" : "Details";
    case "signing":
      return c ? "Locked Terms" : "Approval";
    case "combined":
      return c ? "Signing Request" : "Signed Contribution";
    case "funding_signed":
      return "Signed Contribution";
    default:
      return "Final Record";
  }
}

const blob = (tag: string, extra: Record<string, unknown> = {}) =>
  JSON.stringify({ type: "dash-shared-mn-session", version: 1, network: "test", fixture: tag, ...extra }, null, 2);

/** Messages another party "sent", by what pasting them does. */
export const SH_TEXT = {
  invitation: blob("invitation", { sessionId: SH_PART, revision: 1, stage: "draft" }),
  bobReply: blob("bob-details", { sessionId: SH_COORD, revision: 2, stage: "draft" }),
  carolReply: blob("carol-details", { sessionId: SH_COORD, revision: 3, stage: "draft" }),
  bobApproval: blob("bob-approval", { sessionId: SH_COORD, revision: 4, stage: "signing" }),
  carolApproval: blob("carol-approval", { sessionId: SH_COORD, revision: 4, stage: "signing" }),
  lockedTerms: blob("locked-terms", { sessionId: SH_PART, revision: 3, stage: "signing" }),
  /** Locked again with a 30 Dash penalty after this wallet approved 20. */
  relocked: blob("relocked-terms", { sessionId: SH_PART, revision: 5, stage: "signing" }),
  edited: blob("edited", { sessionId: SH_COORD, revision: 2, stage: "draft", note: "edited" }),
  skipped: blob("skipped-signatures", { sessionId: SH_COORD, revision: 4, stage: "signing", sigs: 2 }),
  noApprovals: blob("no-approvals", { sessionId: SH_COORD, revision: 4, stage: "signing", sigs: "none" }),
  keyChange: JSON.stringify({ kind: "registrar", proTxHash: h("ec"), fixture: "key-change" }),
  dissolveTogether: JSON.stringify({ kind: "dissolve", proTxHash: h("ec"), fixture: "dissolve-together" }),
  standbyLine: fake("0300090001", 380),
  wrongNetwork: JSON.stringify({ type: "dash-shared-mn-session", version: 1, network: "main", fixture: "mainnet" }),
  otherSession: blob("other-session", { sessionId: h("e9"), revision: 2, stage: "draft" }),
  older: blob("older", { sessionId: SH_COORD, revision: 1, stage: "draft" }),
  conflict: blob("conflict", { sessionId: SH_COORD, revision: 9, stage: "draft" }),
  termsChanged: blob("terms-changed", { sessionId: SH_PART, revision: 3, stage: "signing", terms: "other" }),
  badSignature: blob("bad-signature", { sessionId: SH_COORD, revision: 4, stage: "signing" }),
  notParticipant: blob("not-participant", { sessionId: SH_PART, revision: 3, stage: "signing", device: "other" }),
  labels: blob("labels", { sessionId: SH_COORD, revision: 2, stage: "draft", labels: "duplicate" }),
  damaged: '{"type":"dash-shared-mn-session","version":1,',
} as const;

const REFUSALS: Partial<Record<string, [string, number | null]>> = {
  [SH_TEXT.wrongNetwork]: ["network", null],
  [SH_TEXT.otherSession]: ["other_session", null],
  [SH_TEXT.older]: ["older", null],
  [SH_TEXT.conflict]: ["conflict", null],
  [SH_TEXT.termsChanged]: ["terms_changed", null],
  [SH_TEXT.badSignature]: ["bad_signature", 1],
  [SH_TEXT.notParticipant]: ["not_participant", null],
  [SH_TEXT.labels]: ["labels", null],
  [SH_TEXT.damaged]: ["parse", null],
};

export function newSharedWorld(): SharedWorld {
  return { capability: { v24: "active", evonodeShares: "inactive", canSend: true }, sessions: [], nextFile: null, saved: [], standbys: [], unreadable: [], abandoned: [], handoff: null, abandonHandoff: null };
}

function find(world: World, id: string): SharedSession {
  if (!HEX64.test(id)) throw fail("invalid_argument", "a session id is 64 lower-case hex");
  const s = world.shared.sessions.find((x) => x.id === id);
  if (!s) throw fail("masternodes.shared_session_not_found", "no such shared session");
  return s;
}

function put(world: World, s: SharedSession): SharedSession {
  s.updatedAt = Math.max(s.updatedAt + 1, NOW);
  world.shared.sessions = [s, ...world.shared.sessions.filter((x) => x.id !== s.id)];
  return copy(s);
}

const bump = (s: SharedSession) => {
  s.revision += 1;
  s.messageCode = messageCode(`${s.id.slice(6, 10)}${s.revision}`);
};

/** `abandon_shared_session`'s `unsupported` while sending is off. */
function requireSending(world: World) {
  if (!world.shared.capability.canSend) throw fail("masternodes.unsupported", "shared sending is off", { kind: "unsupported", feature: "shared_masternodes", field: null });
}

/** MG's ruling, as dw-app's binding refuses it: before the engine, naming the network. */
function requireSends(world: World) {
  const n = world.launch.network;
  if (n.kind === "mainnet") throw fail("app.shared_send_off", "this version sends shared-masternode transactions on test networks only", { kind: "sharedSendNetwork", network: n.dirName });
}

function requireV24(world: World) {
  if (world.shared.capability.v24 !== "active") throw fail("masternodes.unsupported", "v24 is not active", { kind: "unsupported", feature: "v24", field: null });
}

const outcome = (s: SharedSession) => ({ session: copy(s), message: message(s, messageName(s)) });

function standbyImport(world: World): SharedImport {
  const txid = h("5b");
  const stored = !world.shared.standbys.includes(txid);
  if (stored) world.shared.standbys.push(txid);
  const description: StandbyDescription = {
    standby: { proTxHash: h("ec"), shareIndex: 1, fee: 1_000, penalty: 20 * COIN, fullValidFrom: 1_319_400, full: false, immediate: true, source: "imported", createdAt: NOW },
    variant: "immediate",
    txid,
    penalty: 20 * COIN,
    returns: [
      { shareIndex: 0, amount: 410 * COIN, refundAddress: addr("rA"), mine: false },
      { shareIndex: 1, amount: 280 * COIN - 1_000, refundAddress: addr("rB"), mine: true },
      { shareIndex: 2, amount: 310 * COIN, refundAddress: addr("rC"), mine: false },
    ],
    returnsPrincipal: false,
  };
  return { kind: "standby", description, stored };
}

/** A maintenance request: saved, read only before S6. */
function maintenanceImport(world: World, kind: "key_change" | "dissolve_together"): SharedImport {
  const id = kind === "key_change" ? h("ec1") : h("ed2");
  const existing = world.shared.sessions.find((s) => s.id === id);
  const s =
    existing ??
    session(id, "participant", {
      kind,
      stage: { kind: "signing" },
      next: "approve",
      rows: [shareRow(0, "Share 1", 400), shareRow(1, "Share 2", 300, { mine: true }), shareRow(2, "Share 3", 300)],
      proTxHash: h("ec"),
      fee: kind === "dissolve_together" ? 1_000 : null,
      debit: kind === "dissolve_together" ? 0 : null,
    });
  return { kind: "session", session: put(world, s), changed: !existing, receivedCode: messageCode(id.slice(0, 8)), warnings: [] };
}

function importText(world: World, text: string): SharedImport {
  if (text.length > MAX_TEXT) throw refuse("too_large");
  const refusal = REFUSALS[text];
  if (refusal) throw refuse(...refusal);
  const s = world.shared.sessions;
  const coord = s.find((x) => x.id === SH_COORD);
  const part = s.find((x) => x.id === SH_PART);
  const result = (x: SharedSession, warnings: SharedImport extends infer I ? (I extends { kind: "session"; warnings: infer W } ? W : never) : never = [], changedNow = true): SharedImport => ({
    kind: "session",
    session: put(world, x),
    changed: changedNow,
    receivedCode: messageCode(`${x.id.slice(0, 4)}${text.length}`),
    warnings,
  });
  switch (text) {
    case SH_TEXT.standbyLine:
      return standbyImport(world);
    case SH_TEXT.keyChange:
      return maintenanceImport(world, "key_change");
    case SH_TEXT.dissolveTogether:
      return maintenanceImport(world, "dissolve_together");
    case SH_TEXT.invitation:
      if (part) return result(part, [], false);
      return result(participantSession("invited"));
    case SH_TEXT.bobReply:
    case SH_TEXT.carolReply:
    case SH_TEXT.edited:
    case SH_TEXT.labels: {
      if (!coord) throw refuse("other_session");
      if (coord.stage.kind !== "draft") throw refuse("older");
      const who = text === SH_TEXT.carolReply ? 2 : 1;
      coord.rows[who] = shareRow(who, coord.rows[who].label, coord.rows[who].amount / COIN);
      bump(coord);
      coord.next = coord.rows.every((r) => r.contributed) ? "lock" : "copy";
      return result(coord, text === SH_TEXT.edited ? [{ kind: "edited_message" }] : []);
    }
    case SH_TEXT.bobApproval:
    case SH_TEXT.carolApproval:
    case SH_TEXT.skipped:
    case SH_TEXT.noApprovals: {
      if (!coord) throw refuse("other_session");
      if (coord.stage.kind !== "signing") throw refuse("older");
      if (text === SH_TEXT.noApprovals) return result(coord, [{ kind: "no_approvals" }], false);
      const who = text === SH_TEXT.carolApproval ? 2 : 1;
      coord.rows[who].approved = true;
      if (coord.rows.every((r) => r.approved)) {
        coord.stage = { kind: "combined" };
        coord.next = "copy";
      }
      return result(coord, text === SH_TEXT.skipped ? [{ kind: "skipped_signatures", shares: [2], count: 2 }] : []);
    }
    case SH_TEXT.lockedTerms:
    case SH_TEXT.relocked: {
      if (!part) throw refuse("other_session");
      if (part.stage.kind === "invited") throw refuse("not_participant");
      const relock = text === SH_TEXT.relocked;
      if (relock) {
        part.approvedTerms = part.approvedTerms ?? terms();
        part.terms = terms({ earlyPenalty: 30 * COIN });
        part.rows[0].approved = true;
        part.rows[2].approved = false;
      }
      part.stage = { kind: "signing" };
      part.next = "approve";
      part.consentHash = relock ? h("ce") : h("cd");
      part.revision = relock ? 5 : 3;
      part.messageCode = messageCode(`${part.id.slice(6, 10)}${part.revision}`);
      return result(part);
    }
    default:
      throw refuse("not_shared");
  }
}

export function sharedHandlers({ wallet, grant, useGrant }: { wallet: (w: World, walletId: string) => FixtureWallet; grant: (w: World, id: string, walletId: string, kinds: "masternodeOp"[]) => { grant: { purpose: { kind: string; maxDuffs?: number } } } | null; useGrant: (w: World, id: string) => void }): Handlers {
  const capped = (world: World, walletId: string, grantId: string, cap: number) => {
    const g = grant(world, grantId, walletId, ["masternodeOp"]);
    if (!g) throw fail("platform.grant_invalid", "grant invalid");
    if ((g.grant.purpose.maxDuffs ?? 0) < cap) throw fail("send.grant_exceeded", "the grant doesn't cover the charge");
    useGrant(world, grantId);
  };
  return {
    mn_shared_capability: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return copy(world.shared.capability);
    },
    mn_shared_sessions: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return copy(world.shared.sessions).sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
    },
    mn_shared_unreadable: ({ walletId }, { world }) => {
      wallet(world, walletId);
      return copy(world.shared.unreadable).sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
    },
    mn_shared_session: ({ walletId, session: id }, { world }) => {
      wallet(world, walletId);
      return copy(find(world, id));
    },
    mn_shared_start: ({ walletId, request }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireV24(world);
      const n = request.shares.length;
      const labels = request.shares.map((s) => s.label.trim());
      if (n < 2 || n > 8 || request.shares.some((s) => s.amount < 100 * COIN) || request.shares.reduce((a, s) => a + s.amount, 0) !== SHARE_TOTAL)
        throw fail("invalid_argument", "shares: 2–8 of at least 100 Dash summing to 1,000");
      if (labels.some((l) => !l) || new Set(labels).size !== n) throw fail("invalid_argument", "labels must be non-empty and unique");
      const rows = request.shares.map((s, i) => shareRow(i, s.label.trim(), s.amount / COIN, i === 0 ? { mine: true } : { ownerAddress: null, refundAddress: null, contributed: false }));
      const s = session(SH_COORD, "coordinator", {
        rows,
        terms: terms({ service: request.terms.service, earlyPeriodBlocks: request.terms.earlyPeriodBlocks, earlyPenalty: request.terms.earlyPenalty, operatorReward: request.terms.operatorReward }),
        nodeRunBy: request.nodeRunBy ?? "",
        fee: request.fee,
        debit: request.shares[0].amount + request.fee,
        reserved: [`${h("c0")}-0`],
        createdAt: NOW,
        updatedAt: NOW,
        warnings: request.terms.earlyPeriodBlocks > 0 && request.terms.earlyPenalty === 0 ? [{ kind: "zero_penalty" }] : [],
      });
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_set_fee: ({ walletId, session: id, fee }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const s = find(world, id);
      if (s.role !== "coordinator" || s.stage.kind !== "draft") throw stageRefusal(s.stage.kind);
      if (s.rows.slice(1).some((r) => r.contributed)) throw stageRefusal("replied");
      s.fee = fee;
      s.debit = s.rows[0].amount + fee;
      bump(s);
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_join: ({ walletId, session: id, request }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireV24(world);
      const s = find(world, id);
      if (s.stage.kind !== "invited") throw stageRefusal(s.stage.kind);
      const open = s.rows.filter((r) => !r.ownerAddress).map((r) => r.index);
      if (request.shareIndex === null && open.length !== 1) throw fail("invalid_argument", "name the share to join");
      const index = request.shareIndex ?? open[0]!;
      const row = s.rows[index];
      if (!row || row.ownerAddress) throw fail("invalid_argument", "that share is taken");
      s.rows[index] = { ...shareRow(index, row.label, row.amount / COIN), mine: true };
      s.stage = { kind: "draft" };
      s.next = "copy";
      s.debit = world.shared.capability.canSend && s.id === SH_PART ? row.amount : row.amount;
      s.reserved = [`${h("d0")}-1`];
      bump(s);
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_message: ({ walletId, session: id }, { world }) => {
      wallet(world, walletId);
      const s = find(world, id);
      if (s.stage.kind === "invited") throw stageRefusal("invited");
      if (s.abandonSpend) throw stageRefusal("abandoning");
      const m = message(s, messageName(s));
      world.shared.saved.push(m.text);
      return m;
    },
    mn_shared_lock: ({ walletId, session: id, revision, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireV24(world);
      const s = find(world, id);
      if (s.role !== "coordinator" || s.stage.kind !== "draft") throw stageRefusal(s.stage.kind);
      if (revision !== s.revision) throw refuse("conflict");
      if (!s.rows.every((r) => r.contributed)) throw stageRefusal("incomplete");
      capped(world, walletId, grantId, 0);
      s.stage = { kind: "signing" };
      s.next = "copy";
      s.consentHash = h("cd");
      s.rows[0].approved = true;
      bump(s);
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_unlock: ({ walletId, session: id }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const s = find(world, id);
      if (s.role !== "coordinator" || s.stage.kind !== "signing") throw stageRefusal(s.stage.kind);
      s.stage = { kind: "draft" };
      s.next = "lock";
      s.consentHash = null;
      for (const r of s.rows) r.approved = false;
      bump(s);
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_approve: ({ walletId, session: id, consentHash, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireV24(world);
      const s = find(world, id);
      if (s.role !== "participant" || s.stage.kind !== "signing") throw stageRefusal(s.stage.kind);
      if (!/^[0-9a-fA-F]{64}$/.test(consentHash)) throw fail("invalid_argument", "consent hash: 64 hex");
      if (consentHash.toLowerCase() !== s.consentHash) throw refuse("conflict");
      capped(world, walletId, grantId, 0);
      const mine = s.rows.find((r) => r.mine);
      if (mine) mine.approved = true;
      s.approvedTerms = copy(s.terms);
      s.next = "copy";
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_sign: ({ walletId, session: id, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireSends(world);
      requireV24(world);
      const s = find(world, id);
      if (s.role !== "participant") throw stageRefusal("coordinator");
      if (s.stage.kind !== "combined") throw stageRefusal(s.stage.kind);
      const mine = s.rows.find((r) => r.mine);
      if (!mine?.approved) throw refuse("conflict");
      capped(world, walletId, grantId, s.debit ?? 0);
      mine.fundingSigned = true;
      s.stage = { kind: "funding_signed" };
      s.next = "copy";
      s.discardable = false;
      bump(s);
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_send: ({ walletId, session: id, grant: grantId }, ctx) => {
      const { world } = ctx;
      const w = wallet(world, walletId);
      requireSends(world);
      requireV24(world);
      const s = find(world, id);
      if (s.role !== "coordinator") throw stageRefusal("participant");
      if (s.stage.kind === "combined") {
        if (s.rows.some((r) => !r.mine && !r.fundingSigned)) throw stageRefusal("unsigned");
        capped(world, walletId, grantId, s.debit ?? 0);
        s.rows = s.rows.map((r) => ({ ...r, fundingSigned: true }));
        s.stage = { kind: "sent" };
        s.next = "wait";
        s.txid = h("7e");
        s.proTxHash = h("7e");
        s.discardable = false;
        w.rows = [registrationRow(s.txid, s.debit ?? 0), ...w.rows];
      } else if (s.stage.kind === "sent") {
        if (world.shared.abandoned.includes(s.id)) throw stageRefusal("abandoned");
        capped(world, walletId, grantId, s.debit ?? 0);
      } else throw stageRefusal(s.stage.kind);
      s.lastError = world.shared.handoff;
      const out = outcome(put(world, s));
      changed(ctx);
      return out;
    },
    mn_shared_discard: ({ walletId, session: id }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const lost = world.shared.unreadable.find((x) => x.id === id);
      if (lost) {
        if (!lost.discardable) throw stageRefusal("sent");
        world.shared.unreadable = world.shared.unreadable.filter((x) => x.id !== id);
        changed(ctx);
        return null;
      }
      const s = find(world, id);
      if (!s.discardable) throw stageRefusal(s.stage.kind);
      world.shared.sessions = world.shared.sessions.filter((x) => x.id !== id);
      changed(ctx);
      return null;
    },
    /** The engine's order: the session, its kind, its stage, a coin to spend, then the grant. */
    mn_shared_abandon: ({ walletId, session: id, grant: grantId }, ctx) => {
      const { world } = ctx;
      if (!HEX64.test(id)) throw fail("invalid_argument", "a session id is 64 lower-case hex");
      wallet(world, walletId);
      requireSending(world);
      const lost = world.shared.unreadable.find((x) => x.id === id);
      if (lost?.kind === "dissolve_together") throw fail("invalid_argument", "not a registration session");
      if (lost) throw fail("internal", "a stored shared session does not parse");
      const s = find(world, id);
      if (s.kind === "dissolve_together") throw fail("invalid_argument", "not a registration session");
      const st = s.stage.kind;
      if (s.kind === "key_change") {
        if (s.role !== "coordinator") throw stageRefusal("participant");
        if (st === "confirmed" || st === "conflicted" || st === "dead") throw stageRefusal(st);
      } else if (s.discardable || st === "confirmed" || st === "conflicted") throw stageRefusal(st);
      if (!s.abandonable || s.abandonCharge === null) throw stageRefusal(s.abandonSpend ? "abandoning" : outStage(s.role));
      const fee = s.abandonCharge;
      capped(world, walletId, grantId, fee);
      // The engine's recovery read after the spend's hand-off: the spend recorded (its coins spent),
      // its outcome unknown (no new spend while it may be out), or definitely refused (Try again).
      const outcome = world.shared.abandonHandoff;
      world.shared.abandonHandoff = null;
      const state = outcome ?? "in_history";
      s.abandonSpend = { txid: h(outcome === "refused" ? "a9" : "a8"), state, overdue: false, code: outcome === "refused" ? "send.broadcast_rejected" : outcome === "uncertain" ? "send.broadcast_unknown" : null };
      s.abandonable = outcome !== null;
      s.abandonCharge = outcome === "refused" ? fee : null;
      s.resendAbandonCharge = state === "refused" ? null : fee;
      if (s.next === "copy" || s.next === "send" || s.next === "sign" || s.next === "wait") s.next = "nothing";
      if (st === "sent") world.shared.abandoned.push(s.id);
      const out = put(world, s);
      changed(ctx);
      return out;
    },
    /** The latest abandon spend's saved bytes again, while it may be out: recorded once it reaches the network. */
    mn_shared_resend_abandon: ({ walletId, session: id, grant: grantId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      requireSending(world);
      const s = find(world, id);
      if (!s.abandonSpend) throw stageRefusal(outStage(s.role));
      if (s.abandonSpend.state === "refused") throw stageRefusal("abandon_refused");
      if (s.resendAbandonCharge === null) throw stageRefusal("abandoning");
      capped(world, walletId, grantId, s.resendAbandonCharge);
      s.abandonSpend = { ...s.abandonSpend, state: "in_history", code: null };
      s.abandonable = false;
      const out = put(world, s);
      changed(ctx);
      return out;
    },
    mn_shared_abandon_unreadable: ({ walletId, session: id, grant: grantId }, ctx) => {
      const { world } = ctx;
      if (!HEX64.test(id)) throw fail("invalid_argument", "a session id is 64 lower-case hex");
      wallet(world, walletId);
      requireSending(world);
      const lost = world.shared.unreadable.find((x) => x.id === id);
      const row = lost ?? world.shared.sessions.find((x) => x.id === id);
      if (!row) throw fail("masternodes.shared_session_not_found", "no such shared session");
      if (row.kind === "dissolve_together") throw fail("invalid_argument", "not a registration session");
      if (!lost) throw fail("invalid_argument", "the session reads: abandon_shared_session");
      if (lost.discardable || !lost.abandonable) throw stageRefusal(outStage(lost.role));
      capped(world, walletId, grantId, lost.abandonCharge ?? 0);
      lost.abandonable = false;
      lost.abandonCharge = null;
      lost.updatedAt = Math.max(lost.updatedAt + 1, NOW);
      changed(ctx);
      return copy(lost);
    },
    mn_shared_import: ({ walletId, text }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const out = importText(world, text.trim());
      changed(ctx);
      return out;
    },
    mn_shared_import_file: ({ walletId }, ctx) => {
      const { world } = ctx;
      wallet(world, walletId);
      const f = world.shared.nextFile;
      if (f === null) return null;
      if ("error" in f) {
        if (f.error === "fileNotMessage") throw refuse("not_shared");
        throw fail(f.error, f.error === "app.file_read" ? "could not read the file" : "the file is too large");
      }
      const out = f.texts.map((text) => importText(world, text));
      changed(ctx);
      return out;
    },
  };
}

type Base = (theme: ThemeName | null) => World;

function shBase(base: Base, theme: ThemeName | null, set: (s: SharedWorld, w: World) => void = () => {}): World {
  const w = base(theme);
  w.settings.ui = { ...w.settings.ui, masternodesItem: "on" };
  set(w.shared, w);
  return w;
}

export function sharedScenarios(base: Base) {
  const withSessions = (...sessions: SharedSession[]) => (s: SharedWorld) => (s.sessions = sessions);
  return {
    /** v24 not yet seen: Start is off with Q9's copy (Join still reads messages). */
    "sh-capability-unknown": (t: ThemeName | null) => shBase(base, t, (s) => (s.capability = { ...s.capability, v24: "unknown" })),
    "sh-capability-active": (t: ThemeName | null) => shBase(base, t),
    /** Coordinator, invitation out, nobody in yet. */
    "sh-coordinator-draft": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession())),
    /** Coordinator, Bob in, Carol not. */
    "sh-coordinator-one": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession(["Bob"]))),
    /** Coordinator, every row and contribution in: Lock. */
    "sh-coordinator-ready": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession(["Bob", "Carol"]))),
    /** Coordinator, terms locked, Bob approved. */
    "sh-coordinator-signing": (t: ThemeName | null) =>
      shBase(
        base,
        t,
        withSessions(coordinatorSession(["Bob", "Carol"], { stage: { kind: "signing" }, next: "copy", consentHash: h("cd"), revision: 4, rows: [shareRow(0, "Alice", 400, { mine: true, approved: true }), shareRow(1, "Bob", 300, { approved: true }), shareRow(2, "Carol", 300)] })),
      ),
    /** Coordinator, everyone approved: waiting for signed contributions (signing is S5b). */
    "sh-coordinator-combined": (t: ThemeName | null) =>
      shBase(
        base,
        t,
        withSessions(coordinatorSession(["Bob", "Carol"], { stage: { kind: "combined" }, next: "copy", consentHash: h("cd"), revision: 5, rows: [0, 1, 2].map((i) => shareRow(i, ["Alice", "Bob", "Carol"][i], i ? 300 : 400, { mine: i === 0, approved: true })) })),
      ),
    /** Coordinator, every other contribution signed: Send (S5b). */
    "sh-coordinator-send": (t: ThemeName | null) => shBase(base, t, withSessions(readyToSend())),
    /** The same on mainnet: no Send, the mainnet note in its place. */
    "sh-coordinator-send-mainnet": (t: ThemeName | null) => shBase(base, t, (s, w) => (onMainnet(w), (s.sessions = [readyToSend()]))),
    /** Sent, waiting for its confirmation: Send again. */
    "sh-coordinator-sent": (t: ThemeName | null) => shBase(base, t, withSessions(sentRegistration())),
    /** Out; the next Abandon's hand-off times out, so the spend back may be on the network. */
    "sh-close-out-abandon-uncertain": (t: ThemeName | null) => shBase(base, t, (s) => ((s.sessions = [participantSession("funding_signed")]), (s.abandonHandoff = "uncertain"))),
    /** Out; the network definitely refuses the next Abandon's spend back. */
    "sh-close-out-abandon-refused": (t: ThemeName | null) => shBase(base, t, (s) => ((s.sessions = [participantSession("funding_signed")]), (s.abandonHandoff = "refused"))),
    /** Sent three hours of tip time ago and still unconfirmed: the way out opens expanded (D49). */
    "sh-coordinator-sent-overdue": (t: ThemeName | null) => shBase(base, t, withSessions(sentRegistration({ overdue: true }))),
    /** Sent, the hand-off's outcome unknown. */
    "sh-coordinator-sent-unknown": (t: ThemeName | null) => shBase(base, t, withSessions(sentRegistration({ lastError: "send.broadcast_unknown" }))),
    /** Sent, then abandoned: Send again is refused. */
    "sh-coordinator-sent-abandoned": (t: ThemeName | null) =>
      shBase(base, t, (s) => {
        s.sessions = [sentRegistration()];
        s.abandoned = [SH_COORD];
      }),
    /** Participant at `combined` on mainnet: no Sign. */
    "sh-participant-combined-mainnet": (t: ThemeName | null) => shBase(base, t, (s, w) => (onMainnet(w), (s.sessions = [participantSession("combined")]))),
    "sh-participant-invited": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("invited"))),
    /** Invited with two shares open (Bob's and Dana's): Join waits for a pick. */
    "sh-participant-invited-two": (t: ThemeName | null) =>
      shBase(base, t, withSessions(participantSession("invited", { rows: participantSession("invited").rows.map((r) => (r.index === 1 ? { ...r, ownerAddress: null, refundAddress: null, contributed: false } : r)) }))),
    "sh-participant-joined": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("draft"))),
    "sh-participant-approve": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("signing"))),
    /** Re-locked after this wallet approved: the penalty went 20 → 30 Dash. */
    "sh-participant-approve-diff": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("signing", { approvedTerms: terms(), terms: terms({ earlyPenalty: 30 * COIN }), consentHash: h("ce"), revision: 5 }))),
    "sh-participant-approved": (t: ThemeName | null) =>
      shBase(base, t, withSessions(participantSession("signing", { next: "copy", approvedTerms: terms(), rows: [shareRow(0, "Alice", 400, { approved: true }), shareRow(1, "Bob", 300), shareRow(2, "Dana", 300, { mine: true, approved: true })] }))),
    "sh-participant-combined": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("combined"))),
    "sh-participant-signed": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("funding_signed"))),
    "sh-participant-sent": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("sent"))),
    "sh-participant-dead": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("dead"))),
    "sh-participant-confirmed": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("confirmed", { proTxHash: h("ec") }))),
    /** No change from the coordinator's coins: 0.00004 Dash of dust goes to the fee. */
    "sh-dust-coordinator": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession(["Bob", "Carol"], { debit: 400 * COIN + FEE + 4_000 }))),
    "sh-dust-participant": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("signing", { debit: 300 * COIN + 4_000 }))),
    /** Close before out (Keep / Discard) and out (no question). */
    "sh-close-before": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession(["Bob"]))),
    "sh-close-out": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("funding_signed"))),
    /** Out, with no coin left for this wallet to spend: no Abandon and no line naming it. */
    "sh-signed-out-abandoned": (t: ThemeName | null) => shBase(base, t, withSessions(participantSession("funding_signed", { abandonable: false }))),
    /** Sessions this wallet can't read: one it may discard, one that may be out (Abandon), and one
     * out whose locks the follower already released (not listed). */
    "sh-unreadable": (t: ThemeName | null) =>
      shBase(base, t, (s) => {
        s.sessions = [coordinatorSession(["Bob"])];
        s.unreadable = [unreadable("7c", true, 2), unreadable("9e", false, 3), { ...unreadable("5a", false, 4), abandonable: false, abandonCharge: null, reserved: [] }];
      }),
    /** Both sessions, for the In progress list and the badge. */
    "sh-sessions": (t: ThemeName | null) => shBase(base, t, withSessions(coordinatorSession(["Bob", "Carol"]), participantSession("signing"))),
  };
}

/** The registration in Activity: unconfirmed, so Activity offers to abandon it. */
function registrationRow(txid: string, debit: number): TxRow {
  return {
    txid,
    recordIndex: 0,
    txType: "masternodeRegistration",
    category: "masternode",
    status: { kind: "unconfirmed", confirmations: 0, instantLocked: false, chainLocked: false, maturesIn: null },
    timestamp: secs(FIXTURE_NOW),
    blockHeight: null,
    amount: -debit,
    fee: null,
    address: null,
    label: null,
    countsTowardBalance: true,
    involvesWatchOnly: false,
  };
}

/** Alice coordinates; everyone approved, Bob and Carol signed their contributions. */
function readyToSend(patch: Partial<SharedSession> = {}): SharedSession {
  return coordinatorSession(["Bob", "Carol"], {
    stage: { kind: "combined" },
    next: "send",
    consentHash: h("cd"),
    revision: 6,
    reserved: [`${h("a0")}-0`],
    rows: [shareRow(0, "Alice", 400, { mine: true, approved: true }), shareRow(1, "Bob", 300, { approved: true, fundingSigned: true }), shareRow(2, "Carol", 300, { approved: true, fundingSigned: true })],
    ...patch,
  });
}

function sentRegistration(patch: Partial<SharedSession> = {}): SharedSession {
  return readyToSend({ stage: { kind: "sent" }, next: "wait", discardable: false, txid: h("7e"), proTxHash: h("7e"), rows: readyToSend().rows.map((r) => ({ ...r, fundingSigned: true })), ...patch });
}

function unreadable(byte: string, discardable: boolean, ago: number): UnreadableSession {
  const id = h(byte);
  return withCharge({ id, code: id.slice(0, 6).toUpperCase(), kind: "registration", role: "participant", discardable, abandonable: !discardable, abandonCharge: null, reserved: [`${h("d" + byte[1])}-0`], updatedAt: NOW - ago * 3_600 });
}
