// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's answers for the commands dw-app does not have yet (api/pending.ts): the network
// choice and the Platform addresses (lane E2's contract, m4-dashpay-engine v40 §2.12 and §8) and the
// shielded balance. They are not in the generated `COMMANDS`, so they sit beside `HANDLERS`
// (index.ts looks here second). A test makes one "absent" by setting its field to `null`: the
// handler then rejects as an unknown command does.

import type { AppError } from "../api/generated/AppError";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import {
  PENDING_COMMANDS as PENDING,
  type MoveToCoreOutcome,
  type MoveToCoreQuote,
  type NetworkChoice,
  type NetworkChoiceView,
  type PlatformActivity,
  type PlatformAddressBalance,
} from "../api/pending";
import { checkGrant, HANDLERS, useGrant, wallet, type Ctx } from "./handlers";
import { FIXTURE_NOW } from "./wallet";

/** Platform's credits per duff (dpp's `CREDITS_PER_DUFF`), as the fixture's cost table has it. */
const CREDITS_PER_DUFF = 1000n;
/** What a move costs the fixture's wallet, in credits. */
const MOVE_FEE_CREDITS = 2_000_000n;

/** What the next `platform_move_to_core` comes to: Platform's proved answer, an answer that says
 * nothing (the move stays `pending`), or a refusal. */
export type MoveResult = "done" | "pending" | "refused";

export interface PlatformWorld {
  addresses: { address: string; credits: string }[];
  activity: PlatformActivity[];
  nextMove: MoveResult;
  syncedAt: number | null;
}

export interface PendingWorld {
  /** The network choice: the saved one and whether one was saved; `null`: the command is absent. */
  networkChoice: { chosen: NetworkChoice; saved: boolean } | null;
  /** The shielded balance in duffs; `null`: the command is absent. */
  shielded: number | null;
  /** The Platform addresses; `null`: the commands are absent. */
  platform: PlatformWorld | null;
}

const ADDRESSES = [
  { address: "tdash1kxk5g3wz9mqhz0pnr8x0a2nxzv3xwq7kp6yvr9h5q9u8fz4d7lqsr8a2cd", credits: "30000000000" },
  { address: "tdash1q8u4e2c7y5v9nmp3dfz0ht6l2wrk8s4xj7ga5c9e3v0u6y2pnt4qk5m8zr", credits: "10000000000" },
];

export const newPlatformWorld = (): PlatformWorld => ({
  addresses: ADDRESSES.map((a) => ({ ...a })),
  activity: [
    { id: "pa-2", kind: "incoming", state: "done", credits: "10000000000", address: ADDRESSES[1].address, to: null, expectedDuffs: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 3 * 86_400 },
    { id: "pa-1", kind: "incoming", state: "done", credits: "30000000000", address: ADDRESSES[0].address, to: null, expectedDuffs: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000) - 5 * 86_400 },
  ],
  nextMove: "done",
  syncedAt: Math.floor(FIXTURE_NOW.getTime() / 1000) - 60,
});

export const newPendingWorld = (): PendingWorld => ({
  networkChoice: { chosen: "testnet", saved: true },
  shielded: 125_000_000,
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

function balance(p: PlatformWorld): PlatformAddressBalance {
  const credits = total(p);
  return { credits: String(credits), duffs: Number(credits / CREDITS_PER_DUFF), addresses: [...p.addresses].sort((a, b) => Number(BigInt(b.credits) - BigInt(a.credits))), syncedAt: p.syncedAt, syncing: false };
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
  const credits = moveCredits(platformOf(ctx.world, PENDING.platformMoveToCoreQuote), amount);
  const to = HANDLERS.current_receive_address!({ walletId }, ctx).address;
  return { credits: String(credits), feeCredits: String(MOVE_FEE_CREDITS), expectedDuffs: Number((credits - MOVE_FEE_CREDITS) / CREDITS_PER_DUFF), to, grant: { maxDuffs: 0, maxCredits: String(credits) } };
}

export const PENDING_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  [PENDING.getNetworkChoice]: (_: unknown, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent(PENDING.getNetworkChoice);
    return networkView(world);
  },
  [PENDING.setNetworkChoice]: ({ network }: { network: NetworkChoice }, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent(PENDING.setNetworkChoice);
    if (network !== "mainnet" && network !== "testnet") throw fail("invalid_argument", "not a network", { kind: "reason", reason: "not a network" });
    world.pending.networkChoice = { chosen: network, saved: true };
    return networkView(world);
  },
  [PENDING.shieldedBalance]: (_: unknown, { world }: Ctx) => {
    if (world.pending.shielded === null) throw absent(PENDING.shieldedBalance);
    return world.pending.shielded;
  },
  [PENDING.platformAddresses]: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return balance(platformOf(world, PENDING.platformAddresses));
  },
  [PENDING.platformActivity]: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return platformOf(world, PENDING.platformActivity).activity;
  },
  [PENDING.platformMoveToCoreQuote]: ({ walletId, amount }: { walletId: string; amount: WithdrawAmount }, ctx: Ctx) => quote(ctx, walletId, amount),
  [PENDING.platformMoveToCore]: ({ walletId, amount, grant }: { walletId: string; amount: WithdrawAmount; grant: string }, ctx: Ctx): MoveToCoreOutcome => {
    const { world } = ctx;
    const q = quote(ctx, walletId, amount);
    const p = platformOf(world, PENDING.platformMoveToCore);
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    useGrant(world, grant);
    if (p.nextMove === "refused") throw fail("platform.unavailable", "Platform refused");
    const state = p.nextMove === "pending" ? "pending" : "done";
    const id = `pm-${p.activity.length + 1}`;
    p.activity = [{ id, kind: "moveToCore", state, credits: q.credits, address: null, to: q.to, expectedDuffs: q.expectedDuffs, at: Math.floor(FIXTURE_NOW.getTime() / 1000) }, ...p.activity];
    // A move that is `done` has emptied the addresses it took from; a `pending` one has not been seen yet.
    if (state === "done") p.addresses = [];
    return { id, credits: q.credits, expectedDuffs: q.expectedDuffs, to: q.to, state };
  },
};
