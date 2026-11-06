// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's answers for lane E2's network choice and Platform addresses (contract m4-dashpay-engine
// §2.12 and §8) and, in shielded.ts, lane E1's shielded pool. They sit beside `HANDLERS` (index.ts looks
// here second). A test makes one "absent" by setting its field to `null`: the
// handler then rejects as an unknown command does.

import type { AppError } from "../api/generated/AppError";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { MoveToCoreQuote } from "../api/generated/MoveToCoreQuote";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import type { NetworkChoiceView } from "../api/generated/NetworkChoiceView";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import { isHeldOp } from "../platform/move";
import { classifyAddress } from "./send";
import { authorizeSet, checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import { newRecoveryWorld, RECOVERY_HANDLERS, type RecoveryWorld } from "./recovery";
import { newShieldedWorld, SHIELDED_HANDLERS, type ShieldedWorld } from "./shielded";
import { FIXTURE_NOW } from "./wallet";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), as the fixture's cost table has it. */
const CREDITS_PER_DUFF = 1000n;
/** What a move costs the fixture's wallet, in credits. */
const MOVE_FEE_CREDITS = 2_000_000n;
/** A send's fee estimate (dpp's: 500,000 per input, 6,000,000 for the output, one input); the least
 * a send pays (dpp's minimum output). The most the fee can take is what its source address keeps. */
const SEND_FEE_CREDITS = 6_500_000n;
const SEND_MIN_CREDITS = 500_000n;

/** What the next `platform_move_to_core` comes to: Platform's proved answer, an answer that says
 * nothing (the move stays `pending`), a refusal, a move whose debit is proved but not what
 * arrives (`unproved`: `expectedDuffs` is null), or the refusal of too many separate coins. */
export type MoveResult = "done" | "pending" | "refused" | "unproved" | "tooManyCoins";

/** What the next `platform_send` comes to: proved, unanswered (`pending`), failed by height (it may
 * still land), or refused by Platform. */
export type SendResult = "done" | "pending" | "failed" | "refused";

/** What a send was signed on (the engine's stored record): its recipient, what was asked and the
 * quote's figures, so its exact retry replays them whatever the balances are now. */
export interface SendTerms {
  to: string;
  amount: WithdrawAmount;
  quote: Omit<PlatformSendQuote, "operation" | "retryOf">;
  /** The inputs it signed, as one id (a retry signs these exactly). */
  inputs: string;
}

/** A quote's operation: single use, the held send it retries, and the grant issued for it (the
 * only one that pays it). */
export interface SendOperation {
  quote: PlatformSendQuote;
  used: boolean;
  grant: string | null;
  /** A new send's fee source as quoted, and what it gives (`null`: Max, or a retry on stored terms). */
  source: SendSource | null;
}

/** A send's fee source: the address and what it gives to the inputs. */
export interface SendSource {
  address: string;
  gives: bigint;
  keeps: bigint;
}

/** How a send of `credits` is planned on the balances now: inputs largest first; the fee comes from
 * the first address that keeps enough for it after giving (`null`: the balances cannot carry it). */
function sendPlan(p: PlatformWorld, credits: bigint): SendSource | null {
  let left = credits;
  let source: SendSource | null = null;
  for (const a of [...p.addresses].sort((x, y) => Number(BigInt(y.credits) - BigInt(x.credits)))) {
    const gives = left < BigInt(a.credits) ? left : BigInt(a.credits);
    left -= gives;
    const keeps = BigInt(a.credits) - gives;
    if (source === null && keeps >= SEND_FEE_CREDITS) source = { address: a.address, gives, keeps };
  }
  return left > 0n ? null : source;
}

export interface PlatformWorld {
  addresses: { address: string; credits: string }[];
  activity: PlatformActivity[];
  nextMove: MoveResult;
  nextSend: SendResult;
  /** What the send after the next one comes to (`null`: the same as the next). */
  thenSend: SendResult | null;
  /** A proved send's debit counts the fee estimate (the balances hid the fee taken). */
  estimateSends: boolean;
  /** The terms of each send in the activity (by id). */
  terms: Record<string, SendTerms>;
  /** The quotes' operations (by id) and the next number. */
  operations: Record<string, SendOperation>;
  nextOperation: number;
  /** The lowest unused address, what `platform_receive_address` shows. */
  receive: { address: string; index: number };
  syncedAt: number | null;
  /** Credits on addresses too small to move: in `credits`, not in `movableCredits`. */
  dust: string;
}

export interface PendingWorld {
  /** The network choice: the saved one and whether one was saved; `null`: the command is absent. */
  networkChoice: { chosen: ChosenNetwork; saved: boolean } | null;
  /** The shielded pool (fixture/shielded.ts); `null`: its commands are absent. */
  shielded: ShieldedWorld | null;
  /** The Platform addresses; `null`: the commands are absent. */
  platform: PlatformWorld | null;
  /** The unfinished transfers (fixture/recovery.ts). */
  recovery: RecoveryWorld;
}

/** Someone else's Platform address (P2PKH) the fixture's sends can pay. */
export const PLATFORM_PAYEE = "tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h";

const ADDRESSES = [
  { address: "tdash1kxk5g3wz9mqhz0pnr8x0a2nxzv3xwq7kp6yvr9h5q9u8fz4d7lqsr8a2cd", credits: "30000000000" },
  { address: "tdash1q8u4e2c7y5v9nmp3dfz0ht6l2wrk8s4xj7ga5c9e3v0u6y2pnt4qk5m8zr", credits: "10000000000" },
];

export const newPlatformWorld = (): PlatformWorld => ({
  addresses: ADDRESSES.map((a) => ({ ...a })),
  activity: [
    { id: "pa-2", kind: "incoming", state: "done", credits: "10000000000", address: ADDRESSES[1].address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 3 * 86_400, mayLand: false, estimated: false },
    { id: "pa-1", kind: "incoming", state: "done", credits: "30000000000", address: ADDRESSES[0].address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 5 * 86_400, mayLand: false, estimated: false },
  ],
  nextMove: "done",
  nextSend: "done",
  thenSend: null,
  estimateSends: false,
  terms: {},
  operations: {},
  nextOperation: 1,
  receive: { address: "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0", index: 2 },
  dust: "0",
  syncedAt: Math.floor(FIXTURE_NOW.getTime() / 1000) - 60,
});

export const newPendingWorld = (): PendingWorld => ({
  networkChoice: { chosen: "testnet", saved: true },
  shielded: newShieldedWorld(),
  platform: newPlatformWorld(),
  recovery: newRecoveryWorld(),
});

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });
const absent = (call: string) => ({ code: "platform.not_implemented", message: `the fixture backend has no handler for ${call}`, data: { kind: "call", call } });

/** Whether `kind` is one of the two networks a choice is between: a window on another one was
 * started with `--network`. */
const forced = (kind: string) => kind !== "mainnet" && kind !== "testnet";

function networkView(world: Ctx["world"]): NetworkChoiceView {
  const saved = world.pending.networkChoice!;
  const launched = world.launch.network;
  return { chosen: saved.chosen, saved: saved.saved, launched, forcedByFlag: forced(launched.kind), restartNeeded: !forced(launched.kind) && saved.chosen !== launched.kind };
}

const total = (p: PlatformWorld) => p.addresses.reduce((n, a) => n + BigInt(a.credits), 0n);

/** What a move of everything would take: nothing when it would be refused (it does not clear the fee). */
const movable = (p: PlatformWorld) => (total(p) > MOVE_FEE_CREDITS ? total(p) : 0n);

function balance(p: PlatformWorld): PlatformAddressBalance {
  const credits = total(p) + BigInt(p.dust);
  return { credits: String(credits), movableCredits: String(movable(p)), duffs: Number(movable(p) / CREDITS_PER_DUFF), addresses: [...p.addresses].sort((a, b) => Number(BigInt(b.credits) - BigInt(a.credits))), syncedAt: p.syncedAt, syncing: false };
}

/** The platform state of `world`, or the absent-command rejection. */
function platformOf(world: Ctx["world"], call: string): PlatformWorld {
  if (world.pending.platform === null) throw absent(call);
  return world.pending.platform;
}

/** The credits a move of `amount` takes: all of them, or that many (the quote's refusals). */
function moveCredits(p: PlatformWorld, amount: WithdrawAmount): bigint {
  const available = total(p);
  const credits = amount.kind === "all" ? available : BigInt(amount.credits);
  if (available === 0n || credits <= MOVE_FEE_CREDITS) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(MOVE_FEE_CREDITS + 1n), available: String(available) });
  if (credits > available) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(credits), available: String(available) });
  return credits;
}

function quote(ctx: Ctx, walletId: string, amount: WithdrawAmount): MoveToCoreQuote {
  wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, "platform_move_to_core_quote");
  // As the engine's quote: a send that may still land holds the next move, refused before any passphrase.
  const sending = p.activity.find((a) => a.kind === "send" && isHeldOp(a));
  if (sending) throw moveHeld(sending.id);
  const credits = moveCredits(p, amount);
  const to = HANDLERS.current_receive_address!({ walletId }, ctx).address;
  return { credits: String(credits), feeCredits: String(MOVE_FEE_CREDITS), expectedDuffs: Number((credits - MOVE_FEE_CREDITS) / CREDITS_PER_DUFF), to, grant: { maxDuffs: 0, maxCredits: String(credits) } };
}

/** The engine's recipient check (m4 §2.12): a Platform address of this network that is not one of
 * the wallet's own. */
function recipient(world: Ctx["world"], p: PlatformWorld, to: string): string {
  const network = world.launch.network.dirName;
  const t = to.trim();
  const refuse = (reason: string) => fail("credits.recipient_invalid", `recipient invalid: ${reason}`, { kind: "reason", reason });
  const c = classifyAddress(network, t);
  if (c.kind === "core" || c.kind === "shielded") throw refuse(c.kind);
  if (c.kind === "invalid") throw refuse(classifyAddress(network === "mainnet" ? "testnet" : "mainnet", t).kind === "invalid" ? "malformed" : "network");
  if (t === p.receive.address || p.addresses.some((a) => a.address === t)) throw refuse("own");
  return t;
}

/** The terms of a nominal send of `credits` to `to` as the fixture's quote plans one: one input (the
 * largest address), which pays the fee, so the fee can take all it keeps after the send. */
export function sendTerms(to: string, credits: bigint, inputs = "in-1", source = BigInt(ADDRESSES[0].credits), feeSource = ADDRESSES[0].address): SendTerms {
  const kept = source - credits;
  return {
    to,
    amount: { kind: "credits", credits: String(credits) },
    inputs,
    quote: { credits: String(credits + kept), amountCredits: String(credits), feeCredits: String(SEND_FEE_CREDITS), feeSourceCredits: String(kept), feeSource, feeFromAmount: false, to, grant: { maxDuffs: 0, maxCredits: String(credits + kept) } },
  };
}

const sameAmount = (a: WithdrawAmount, b: WithdrawAmount) => (a.kind === "all" ? b.kind === "all" : b.kind === "credits" && a.credits === b.credits);

/** Whether `to` and `amount` are the exact retry of the send with these terms (the engine's `retried_by`). */
const retriedBy = (t: SendTerms | undefined, to: string, amount: WithdrawAmount) => !!t && t.to === to && sameAmount(t.amount, amount);

/** The held send (failed by height, may still land) `to` and `amount` is the exact retry of. */
function retryTarget(p: PlatformWorld, to: string, amount: WithdrawAmount): PlatformActivity | null {
  return p.activity.find((a) => a.kind === "send" && a.state === "failed" && a.mayLand && retriedBy(p.terms[a.id], to, amount)) ?? null;
}

const moveHeld = (id: string) => fail("credits.move_pending", "a move is pending", { kind: "artifact", artifact: id });

/** What holds a send (`credits.move_pending`), as the engine: a move or send Platform has not
 * answered or that may still land, unless this is the exact retry of the only such send and its
 * retries (one group: they share terms). Returns the send it retries. */
function holdFor(p: PlatformWorld, to: string, amount: WithdrawAmount): PlatformActivity | null {
  const held = p.activity.filter(isHeldOp);
  if (held.length === 0) return null;
  const retry = retryTarget(p, to, amount);
  const blocking = held.find((a) => !retry || p.terms[a.id] !== p.terms[retry.id]);
  if (blocking) throw moveHeld(blocking.id);
  return retry;
}

/** Settles the group of the send `done` (it executed): every other member that still holds is
 * replaced, as the engine's `settle` does once a member's nonces are taken. */
function replaceOthers(p: PlatformWorld, done: string): void {
  const terms = p.terms[done];
  p.activity = p.activity.map((a) => (a.id !== done && p.terms[a.id] === terms && isHeldOp(a) ? { ...a, state: "replaced" as const, mayLand: false } : a));
}

/** The send a quote names with `retryOf`, whatever it became: it must be a send of these terms. */
function namedRetry(p: PlatformWorld, id: string, to: string, amount: WithdrawAmount): PlatformActivity {
  const row = p.activity.find((a) => a.id === id && a.kind === "send");
  if (!row) throw fail("invalid_argument", "no send has that id", { kind: "reason", reason: "no send has that id" });
  if (!retriedBy(p.terms[id], to, amount)) throw fail("invalid_argument", "not that send's retry", { kind: "reason", reason: "not that send's retry: the same address and amount" });
  if (row.state === "pending") throw moveHeld(row.id);
  return row;
}

const unknownOperation = () => fail("invalid_argument", "unknown or used operation", { kind: "reason", reason: "unknown or used operation" });

function sendQuote(ctx: Ctx, walletId: string, to: string, amount: WithdrawAmount, retryOf: string | null): PlatformSendQuote {
  wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, "platform_send_quote");
  const address = recipient(ctx.world, p, to);
  const retry = retryOf === null ? holdFor(p, address, amount) : namedRetry(p, retryOf, address, amount);
  const operation = `op-${p.nextOperation++}`;
  const keep = (q: Omit<PlatformSendQuote, "operation" | "retryOf">, retryOf: string | null, source: SendSource | null = null): PlatformSendQuote => {
    const quote = { ...q, operation, retryOf };
    p.operations[operation] = { quote, used: false, grant: null, source };
    return quote;
  };
  // The exact retry is quoted on the held send's stored terms (a settled one is answered when sent).
  if (retry) return keep(p.terms[retry.id].quote, retry.id);
  const available = total(p);
  const short = (needed: bigint) => fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(needed), available: String(available) });
  const grant = (credits: bigint) => ({ maxDuffs: 0, maxCredits: String(credits) });
  if (amount.kind === "all") {
    // Max: everything, the output pays the fee.
    if (available < SEND_MIN_CREDITS + SEND_FEE_CREDITS) throw short(SEND_MIN_CREDITS + SEND_FEE_CREDITS);
    return keep({ credits: String(available), amountCredits: String(available), feeCredits: String(SEND_FEE_CREDITS), feeSourceCredits: null, feeSource: null, feeFromAmount: true, to: address, grant: grant(available) }, null);
  }
  const credits = BigInt(amount.credits);
  if (credits < SEND_MIN_CREDITS) throw fail("credits.send_out_of_range", "below the minimum", { kind: "creditMinimum", min: String(SEND_MIN_CREDITS) });
  const source = sendPlan(p, credits);
  if (source === null) throw short(credits + SEND_FEE_CREDITS);
  const debit = credits + source.keeps;
  return keep({ credits: String(debit), amountCredits: String(credits), feeCredits: String(SEND_FEE_CREDITS), feeSourceCredits: String(source.keeps), feeSource: source.address, feeFromAmount: false, to: address, grant: grant(debit) }, null, source);
}

/** A send's outcome, from its activity row and terms. */
function sendOutcome(p: PlatformWorld, row: PlatformActivity): PlatformSendOutcome {
  const t = p.terms[row.id];
  return {
    id: row.id,
    credits: row.credits,
    amountCredits: t?.quote.amountCredits ?? row.credits,
    feeFromAmount: t?.quote.feeFromAmount ?? false,
    to: row.to,
    state: row.state,
    mayLand: row.mayLand,
    feeCredits: row.feeCredits,
    estimated: row.estimated,
  };
}

/** A held send lands meanwhile (Platform takes it after all): its row is `done`, debited as a proved send. */
export function landSend(p: PlatformWorld, id: string) {
  const row = p.activity.find((a) => a.id === id);
  const t = p.terms[id];
  if (!row || !t) return;
  const fee = t.quote.feeFromAmount ? null : t.quote.feeCredits;
  const debit = BigInt(t.quote.amountCredits) + BigInt(fee ?? 0);
  p.activity = p.activity.map((a) => (a.id === id ? { ...a, state: "done" as const, credits: String(debit), feeCredits: fee, mayLand: false } : a));
  replaceOthers(p, id);
  spend(p, debit);
}

/** Takes `credits` from the addresses, largest first (a proved send). */
function spend(p: PlatformWorld, credits: bigint) {
  let left = credits;
  p.addresses = [...p.addresses]
    .sort((a, b) => Number(BigInt(b.credits) - BigInt(a.credits)))
    .map((a) => {
      const take = left < BigInt(a.credits) ? left : BigInt(a.credits);
      left -= take;
      return { ...a, credits: String(BigInt(a.credits) - take) };
    })
    .filter((a) => a.credits !== "0");
}

export const PENDING_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  get_network_choice: (_: unknown, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent("get_network_choice");
    return networkView(world);
  },
  set_network_choice: ({ network }: { network: ChosenNetwork }, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent("set_network_choice");
    if (network !== "mainnet" && network !== "testnet") throw fail("invalid_argument", "not a network", { kind: "reason", reason: "not a network" });
    world.pending.networkChoice = { chosen: network, saved: true };
    return networkView(world);
  },
  ...SHIELDED_HANDLERS,
  ...RECOVERY_HANDLERS,
  platform_addresses: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return balance(platformOf(world, "platform_addresses"));
  },
  platform_activity: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return platformOf(world, "platform_activity").activity;
  },
  platform_addresses_sync: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    const p = platformOf(world, "platform_addresses_sync");
    p.syncedAt = Math.floor(FIXTURE_NOW.getTime() / 1000);
    return balance(p);
  },
  platform_receive_address: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return platformOf(world, "platform_receive_address").receive;
  },
  platform_send_quote: ({ walletId, to, amount, retryOf }: { walletId: string; to: string; amount: WithdrawAmount; retryOf?: string | null }, ctx: Ctx) => sendQuote(ctx, walletId, to, amount, retryOf ?? null),
  // As the engine: a `platformOp` grant of the quote's credits, the only grant its operation's send takes.
  authorize_platform_send: ({ walletId, operation, credential }: { walletId: string; operation: string; credential: CredentialSpec }, ctx: Ctx): AuthGrant => {
    const { world } = ctx;
    wallet(world, walletId);
    const op = platformOf(world, "authorize_platform_send").operations[operation];
    if (!op || op.used) throw unknownOperation();
    const [grant] = authorizeSet(world, [{ kind: "platformOp", maxDuffs: 0, maxCredits: op.quote.credits }], walletId, credential);
    // As the host: the grant an earlier call bound is revoked.
    if (op.grant) useGrant(world, op.grant);
    op.grant = grant.id;
    return grant;
  },
  platform_send: ({ walletId, operation, grant }: { walletId: string; operation: string; grant: string }, ctx: Ctx): PlatformSendOutcome => {
    const { world } = ctx;
    wallet(world, walletId);
    const p = platformOf(world, "platform_send");
    const op = p.operations[operation];
    // An operation is the session's and single use: unknown or used is quoted again.
    if (!op || op.used) throw unknownOperation();
    // Only the grant issued for it pays it: any other is refused unused, and the operation stays.
    if (op.grant !== grant) throw fail("platform.grant_invalid", "grant invalid");
    op.used = true;
    const q = op.quote;
    // Only a fresh grant pays someone else: a lease id is no grant here.
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    const held = p.activity.filter(isHeldOp);
    const target = q.retryOf === null ? null : (p.activity.find((a) => a.id === q.retryOf) ?? null);
    if (target === null) {
      // A new send, or a hold that appeared after its quote.
      if (held.length > 0) throw moveHeld(held[0].id);
      // As the engine: a new send takes its fee as its Confirm showed, or is quoted again.
      const now = op.source && sendPlan(p, BigInt(q.amountCredits));
      if (op.source && (now?.address !== op.source.address || now.gives !== op.source.gives))
        throw fail("credits.quote_changed", "the balances changed since this send was quoted");
    } else if (target.state === "pending") throw moveHeld(target.id);
    else if (!isHeldOp(target)) {
      // It landed, was replaced by its identical retry, or can never land: its settled outcome, nothing signed or paid.
      const landed = target.state === "replaced" ? p.activity.find((a) => a.state === "done" && p.terms[a.id] === p.terms[target.id]) : undefined;
      return sendOutcome(p, landed ?? target);
    }
    useGrant(world, grant);
    if (p.nextSend === "refused") throw fail("platform.unavailable", "the fixture refuses the send");
    const state = p.nextSend;
    if (p.thenSend !== null) {
      p.nextSend = p.thenSend;
      p.thenSend = null;
    }
    const id = `ps-${p.activity.length + 1}`;
    // As the engine: a proved send's `credits` is the debit (the fee included), an unsettled one's the amount.
    // A Max send's output paid the fee (no figure); with `estimateSends` the balances hid it, so the debit counts the estimate.
    const proved = !q.feeFromAmount && !p.estimateSends;
    const feeCredits = state === "done" && proved ? q.feeCredits : null;
    const credits = state === "done" ? String(BigInt(q.amountCredits) + (q.feeFromAmount ? 0n : BigInt(q.feeCredits))) : q.amountCredits;
    const row: PlatformActivity = {
      id,
      kind: "send",
      state,
      credits,
      address: null,
      to: q.to,
      expectedDuffs: null,
      feeCredits,
      at: Math.floor(FIXTURE_NOW.getTime() / 1000),
      mayLand: state !== "done",
      estimated: state === "done" && !q.feeFromAmount && !proved,
    };
    p.activity = [row, ...p.activity];
    p.terms[id] = target ? p.terms[target.id] : { to: q.to, amount: q.feeFromAmount ? { kind: "all" } : { kind: "credits", credits: q.amountCredits }, inputs: `in-${id}`, quote: (({ operation: _o, retryOf: _r, ...rest }) => rest)(q) };
    // Of a group at most one executes: a retry replaces the sends it re-signed only once it is done.
    // Until a member settles, every member may still land as itself.
    if (state === "done") {
      replaceOthers(p, id);
      spend(p, BigInt(credits));
    }
    return sendOutcome(p, row);
  },
  platform_move_to_core_quote: ({ walletId, amount }: { walletId: string; amount: WithdrawAmount }, ctx: Ctx) => quote(ctx, walletId, amount),
  platform_move_to_core: ({ walletId, amount, to, grant }: { walletId: string; amount: WithdrawAmount; to: string; grant: string }, ctx: Ctx): MoveToCoreOutcome => {
    const { world } = ctx;
    const q = quote(ctx, walletId, amount);
    // Only to one of the wallet's own receiving addresses (the quote's).
    if (to !== q.to) throw fail("invalid_argument", "not an address of this wallet", { kind: "reason", reason: "not an address of this wallet" });
    const p = platformOf(world, "platform_move_to_core");
    // A send that may still land, or a move Platform has not answered, holds the next move (it waits until it
    // settles). A move that failed by height does not: the next one signs at the same inputs and frees it.
    const holding = p.activity.find((a) => isHeldOp(a) && (a.kind === "send" || a.state === "pending"));
    if (holding) throw moveHeld(holding.id);
    const supersedes = p.activity.filter((a) => a.kind === "moveToCore" && isHeldOp(a)).map((a) => a.id);
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    useGrant(world, grant);
    if (p.nextMove === "refused") throw fail("platform.unavailable", "the fixture refuses the move");
    if (p.nextMove === "tooManyCoins") throw fail("invalid_argument", "more than 50 separate balances", { kind: "reason", reason: "Your Platform addresses hold too many separate balances to move at once. Consolidate coins first" });
    const state = p.nextMove === "pending" ? "pending" : "done";
    const expectedDuffs = p.nextMove === "unproved" ? null : q.expectedDuffs;
    const id = `pm-${p.activity.length + 1}`;
    const feeCredits = state === "done" ? q.feeCredits : null;
    // Of a group of moves at the same inputs at most one executes: the new one replaces the held ones.
    p.activity = [
      { id, kind: "moveToCore", state, credits: q.credits, address: null, to: q.to, expectedDuffs, feeCredits, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: state === "pending", estimated: false },
      ...p.activity.map((a) => (supersedes.includes(a.id) ? { ...a, state: "replaced" as const, mayLand: false } : a)),
    ];
    // A move that is `done` has emptied the addresses it took from; a `pending` one has not been seen yet.
    if (state === "done") p.addresses = [];
    return { id, credits: q.credits, expectedDuffs, to: q.to, state, feeCredits };
  },
};
