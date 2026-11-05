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
import { checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import { newRecoveryWorld, RECOVERY_HANDLERS, type RecoveryWorld } from "./recovery";
import { newShieldedWorld, SHIELDED_HANDLERS, type ShieldedWorld } from "./shielded";
import { FIXTURE_NOW } from "./wallet";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), as the fixture's cost table has it. */
const CREDITS_PER_DUFF = 1000n;
/** What a move costs the fixture's wallet, in credits. */
const MOVE_FEE_CREDITS = 2_000_000n;

/** What the next `platform_move_to_core` comes to: Platform's proved answer, an answer that says
 * nothing (the move stays `pending`), a refusal, a move whose debit is proved but not what
 * arrives (`unproved`: `expectedDuffs` is null), or the refusal of too many separate coins. */
export type MoveResult = "done" | "pending" | "refused" | "unproved" | "tooManyCoins";

export interface PlatformWorld {
  addresses: { address: string; credits: string }[];
  activity: PlatformActivity[];
  nextMove: MoveResult;
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
  const credits = moveCredits(platformOf(ctx.world, "platform_move_to_core_quote"), amount);
  const to = HANDLERS.current_receive_address!({ walletId }, ctx).address;
  return { credits: String(credits), feeCredits: String(MOVE_FEE_CREDITS), expectedDuffs: Number((credits - MOVE_FEE_CREDITS) / CREDITS_PER_DUFF), to, grant: { maxDuffs: 0, maxCredits: String(credits) } };
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
