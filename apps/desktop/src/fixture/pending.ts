// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's answers for lane E2's network choice and Platform addresses (contract m4-dashpay-engine
// §2.12 and §8) and, in shielded.ts, lane E1's shielded pool. They sit beside `HANDLERS` (index.ts looks
// here second). A test makes one "absent" by setting its field to `null`: the
// handler then rejects as an unknown command does.

import type { AppError } from "../api/generated/AppError";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { MoveToCoreQuote } from "../api/generated/MoveToCoreQuote";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import type { NetworkChoiceView } from "../api/generated/NetworkChoiceView";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import { classifyAddress } from "./send";
import { checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import { newShieldedWorld, SHIELDED_HANDLERS, type ShieldedWorld } from "./shielded";
import { FIXTURE_NOW } from "./wallet";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), as the fixture's cost table has it. */
const CREDITS_PER_DUFF = 1000n;
/** What a move costs the fixture's wallet, in credits. */
const MOVE_FEE_CREDITS = 2_000_000n;
/** A send's fee estimate (dpp's: 500,000 per input, 6,000,000 for the output, one input) and the
 * library's band on it; the least a send pays (dpp's minimum output). */
const SEND_FEE_CREDITS = 6_500_000n;
const SEND_FEE_BAND = 3n;
const SEND_MIN_CREDITS = 500_000n;

/** What the next `platform_move_to_core` comes to: Platform's proved answer, an answer that says
 * nothing (the move stays `pending`), a refusal, a move whose debit is proved but not what
 * arrives (`unproved`: `expectedDuffs` is null), or the refusal of too many separate coins. */
export type MoveResult = "done" | "pending" | "refused" | "unproved" | "tooManyCoins";

/** What the next `platform_send` comes to: proved, unanswered (`pending`), failed by height (it may
 * still land), or refused by Platform. */
export type SendResult = "done" | "pending" | "failed" | "refused";

export interface PlatformWorld {
  addresses: { address: string; credits: string }[];
  activity: PlatformActivity[];
  nextMove: MoveResult;
  nextSend: SendResult;
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
    { id: "pa-2", kind: "incoming", state: "done", credits: "10000000000", address: ADDRESSES[1].address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 3 * 86_400 },
    { id: "pa-1", kind: "incoming", state: "done", credits: "30000000000", address: ADDRESSES[0].address, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 5 * 86_400 },
  ],
  nextMove: "done",
  nextSend: "done",
  receive: { address: "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0", index: 2 },
  dust: "0",
  syncedAt: Math.floor(FIXTURE_NOW.getTime() / 1000) - 60,
});

export const newPendingWorld = (): PendingWorld => ({
  networkChoice: { chosen: "testnet", saved: true },
  shielded: newShieldedWorld(),
  platform: newPlatformWorld(),
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
  const credits = moveCredits(platformOf(ctx.world, "platform_move_to_core_quote"), amount);
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

/** What holds the next send (`credits.move_pending`), as the engine: a send or move Platform has
 * not confirmed, or a send failed by height (the fixture's failed sends may all still land) unless
 * this is its exact retry (same recipient and what it may take). */
function held(p: PlatformWorld, q: PlatformSendQuote) {
  const h = p.activity.find(
    (a) => ((a.kind === "moveToCore" || a.kind === "send") && a.state === "pending") || (a.kind === "send" && a.state === "failed" && (a.to !== q.to || a.credits !== q.credits)),
  );
  if (h) throw fail("credits.move_pending", "a move is pending", { kind: "artifact", artifact: h.id });
}

function sendQuote(ctx: Ctx, walletId: string, to: string, amount: WithdrawAmount): PlatformSendQuote {
  wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, "platform_send_quote");
  const address = recipient(ctx.world, p, to);
  const available = total(p);
  const band = SEND_FEE_CREDITS * SEND_FEE_BAND;
  const short = (needed: bigint) => fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(needed), available: String(available) });
  const grant = (credits: bigint) => ({ maxDuffs: 0, maxCredits: String(credits) });
  if (amount.kind === "all") {
    // Max: everything, the output pays the fee.
    if (available < SEND_MIN_CREDITS + band) throw short(SEND_MIN_CREDITS + band);
    return { credits: String(available), amountCredits: String(available), feeCredits: String(SEND_FEE_CREDITS), maxFeeCredits: String(band), feeFromAmount: true, to: address, grant: grant(available) };
  }
  const credits = BigInt(amount.credits);
  if (credits < SEND_MIN_CREDITS) throw fail("credits.send_out_of_range", "below the minimum", { kind: "creditMinimum", min: String(SEND_MIN_CREDITS) });
  if (credits + band > available) throw short(credits + band);
  return { credits: String(credits + band), amountCredits: String(credits), feeCredits: String(SEND_FEE_CREDITS), maxFeeCredits: String(band), feeFromAmount: false, to: address, grant: grant(credits + band) };
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
  platform_send_quote: ({ walletId, to, amount }: { walletId: string; to: string; amount: WithdrawAmount }, ctx: Ctx) => sendQuote(ctx, walletId, to, amount),
  platform_send: ({ walletId, to, amount, grant }: { walletId: string; to: string; amount: WithdrawAmount; grant: string }, ctx: Ctx): PlatformSendOutcome => {
    const { world } = ctx;
    const q = sendQuote(ctx, walletId, to, amount);
    const p = platformOf(world, "platform_send");
    held(p, q);
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    useGrant(world, grant);
    if (p.nextSend === "refused") throw fail("platform.unavailable", "the fixture refuses the send");
    const state = p.nextSend;
    const id = `ps-${p.activity.length + 1}`;
    // As the engine: a proved send's `credits` is the debit (the fee included), an unsettled one's what it may take.
    const feeCredits = state === "done" && !q.feeFromAmount ? q.feeCredits : null;
    const credits = state === "done" ? String(BigInt(q.amountCredits) + BigInt(feeCredits ?? 0)) : q.credits;
    p.activity = [{ id, kind: "send", state, credits, address: null, to: q.to, expectedDuffs: null, feeCredits, at: Math.floor(FIXTURE_NOW.getTime() / 1000) }, ...p.activity];
    if (state === "done") spend(p, BigInt(credits));
    return { id, credits, to: q.to, state, feeCredits };
  },
  platform_move_to_core_quote: ({ walletId, amount }: { walletId: string; amount: WithdrawAmount }, ctx: Ctx) => quote(ctx, walletId, amount),
  platform_move_to_core: ({ walletId, amount, to, grant }: { walletId: string; amount: WithdrawAmount; to: string; grant: string }, ctx: Ctx): MoveToCoreOutcome => {
    const { world } = ctx;
    const q = quote(ctx, walletId, amount);
    // Only to one of the wallet's own receiving addresses (the quote's).
    if (to !== q.to) throw fail("invalid_argument", "not an address of this wallet", { kind: "reason", reason: "not an address of this wallet" });
    const p = platformOf(world, "platform_move_to_core");
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
    p.activity = [{ id, kind: "moveToCore", state, credits: q.credits, address: null, to: q.to, expectedDuffs, feeCredits, at: Math.floor(FIXTURE_NOW.getTime() / 1000) }, ...p.activity];
    // A move that is `done` has emptied the addresses it took from; a `pending` one has not been seen yet.
    if (state === "done") p.addresses = [];
    return { id, credits: q.credits, expectedDuffs, to: q.to, state, feeCredits };
  },
};
