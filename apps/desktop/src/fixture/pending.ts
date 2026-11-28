// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's answers for lane E2's network choice and Platform addresses (contract m4-dashpay-engine
// §2.12 and §8) and, in shielded.ts, lane E1's shielded pool. They sit beside `HANDLERS` (index.ts looks
// here second). A test makes one "absent" by setting its field to `null`: the
// handler then rejects as an unknown command does.

import type { AppError } from "../api/generated/AppError";
import type { CommandArgs } from "../api/generated/commands";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { FundAmount } from "../api/generated/FundAmount";
import type { PaymentSourceName } from "../api/generated/PaymentSourceName";
import type { WithdrawAmount } from "../api/generated/WithdrawAmount";
import type { MoveToCoreOutcome } from "../api/generated/MoveToCoreOutcome";
import type { MoveToCoreQuote } from "../api/generated/MoveToCoreQuote";
import type { ChosenNetwork } from "../api/generated/ChosenNetwork";
import type { NetworkChoiceView } from "../api/generated/NetworkChoiceView";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { NetworkSwitch } from "../api/generated/NetworkSwitch";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import type { PlatformAddressRow } from "../api/generated/PlatformAddressRow";
import type { PlatformCancelQuote } from "../api/generated/PlatformCancelQuote";
import type { PendingPlatformFund } from "../api/generated/PendingPlatformFund";
import type { PlatformFundOutcome } from "../api/generated/PlatformFundOutcome";
import type { PlatformFundQuote } from "../api/generated/PlatformFundQuote";
import type { PlatformSendOutcome } from "../api/generated/PlatformSendOutcome";
import type { PlatformSendQuote } from "../api/generated/PlatformSendQuote";
import type { ContactPayQuoteInfo } from "../api/generated/ContactPayQuoteInfo";
import type { PayoutStateName } from "../api/generated/PayoutStateName";
import type { TxCounterparty } from "../api/generated/TxCounterparty";
import { PLATFORM_ADDRESS_SENDS } from "../platform/mainnetOff";
import { heldExitOp, isHeldCancel, isHeldOp, isHeldSend } from "../platform/move";
import { classifyAddress } from "./send";
import { authorizeSet, checkGrant, HANDLERS, landHeldTopUp, openedNetwork, payeeOfPayout, payoutOnContact, restoredNetwork, useGrant, wallet, type Ctx } from "./handlers";
import { applyFund, CREDITS_PER_DUFF, FUND_FEE_CREDITS, FUND_MIN_DUFFS, fundCredits, newRecoveryWorld, RECOVERY_HANDLERS, REMAINDER_HANDLERS, type RecoveryWorld } from "./recovery";
import { SHIELD_LOCK_INPUTS } from "./coinjoin";
import { coreMaxPlan, lockInputs, newShieldedWorld, SHIELDED_HANDLERS, type ShieldedWorld } from "./shielded";
import { restarted } from "./registration";
import { buildWorld, startOverInNewFolder, type World } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";
import { restoreFailure, STORE_SCREENS, storeRefusedError, storeRestoreError } from "./errors";
import type { RestoreStatus } from "../api/generated/RestoreStatus";
import { FIXTURE_DATA_DIR } from "./settings";

/** What a move costs the fixture's wallet, in credits. */
const MOVE_FEE_CREDITS = 2_000_000n;
/** A send's fee estimate (dpp's: 500,000 per input, 6,000,000 for the output, one input); the least
 * a send pays (dpp's minimum output). The fee is taken from its source address, whatever it holds then. */
const SEND_FEE_CREDITS = 6_500_000n;
const SEND_MIN_CREDITS = 500_000n;
/** The least a held move or contact payment has a cancel at: the cancel's output, less three times its
 * fee estimate, must reach a send's least (the engine's `cancel_plan`). */
const CANCEL_MIN_CREDITS = SEND_MIN_CREDITS + 3n * SEND_FEE_CREDITS;
/** A contact payout's fee estimate, and Platform's withdrawal range (fixture/topup.ts's). */
const PAYOUT_FEE_CREDITS = 2_000_000n;
const PAYOUT_MIN_CREDITS = 1_190_000n;
const PAYOUT_MAX_CREDITS = 50_000_000_000_000n;
/** What the asset unlock keeps back on Core, in duffs. */
const UNLOCK_FEE_DUFFS = 226;

/** What the next `platform_move_to_core` comes to: Platform's proved answer, an answer that says
 * nothing (the move stays `pending`), a refusal, a move whose debit is proved but not what
 * arrives (`unproved`: `expectedDuffs` is null), the refusal of too many separate coins, or the exit of a
 * held top-up, send, move or payment that executed in its place (`lost`: the move is `failed` and moves
 * nothing). Beside a held cancel, `lost` is the cancelled move or payment going through, `lostToCancel`
 * the cancel. */
export type MoveResult = "done" | "pending" | "refused" | "unproved" | "tooManyCoins" | "lost" | "lostToCancel";

/** What the next `platform_fund` comes to: applied, locked with its proof still missing, locked and
 * possibly already applied (`checking`), locked and refused by Platform (`notApplied`), or locked
 * with its approval ended before the apply (`approvalEnded`: `notApplied` with `platform.lease_revoked`). */
export type FundResult = "done" | "proofWaiting" | "checking" | "notApplied" | "approvalEnded";

/** What the next `resume_platform_fund` comes to: applied, or refused `platform.needs_grant` before
 * its apply, the lock still waiting (`needsGrant`). */
export type ResumeResult = "done" | "needsGrant";

/** The Core fee bound of the lock, in duffs. */
const FUND_LOCK_FEE_DUFFS = 1_000;

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
  /** What the next "Add from Dash wallet" comes to. */
  nextFund: FundResult;
  /** What the next top-up of an identity from these addresses comes to: done, unanswered
   * (`pending`), failed by height and able to land still (`held`), or never executing (`failed`). */
  nextTopUp: "done" | "pending" | "held" | "failed";
  /** The addresses each top-up from Platform not done yet signed at (by id): its exit moves those. */
  heldInputs?: Record<string, string[]>;
  /** What each of those claimed for which identity: it lands with that if it executes in its exit's place. */
  heldTopUps?: Record<string, { walletId: string; identity: string; inputs: PlatformAddressRow[]; credits: string }>;
  /** What the next Finish of one of those adds comes to. */
  nextResume: ResumeResult;
  /** The locks of "Add from Dash wallet" not applied yet. */
  funds: PendingPlatformFund[];
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
  /** `false`: the network's protocol version has no address funds transitions yet (below 11). */
  sendsActive?: boolean;
  /** What the next payment to a contact from this balance comes to (C-funding D-C5). */
  nextPayout: PayoutStateName;
  /** The next payment's reply is lost after the engine recorded it (an `internal` error). */
  loseReply?: boolean;
  /** Platform's structure pre-check refuses every move, send and top-up from these addresses before
   * anything is signed: `rulesUnsupported` while the network runs a newer protocol version. */
  preCheck?: "refused" | "rulesUnsupported";
  /** The same refusal of cancels only: a held cancel's exact re-send under rules that refuse its terms. */
  cancelPreCheck?: "refused" | "rulesUnsupported";
}

/** As the engine, after the holds and before the grant: the pre-check's refusal, its consensus error in
 * the message (the log), never in the data. */
export function preCheck(p: PlatformWorld, refusal = p.preCheck) {
  if (!refusal) return;
  const code = refusal === "rulesUnsupported" ? "credits.rules_unsupported" : "credits.preflight_refused";
  throw fail(code, "FeeStrategyTooManyStepsError: Fee strategy has 1 steps, maximum allowed is 0");
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
  nextFund: "done",
  nextTopUp: "done",
  nextResume: "done",
  funds: [],
  thenSend: null,
  estimateSends: false,
  terms: {},
  operations: {},
  nextOperation: 1,
  receive: { address: "tdash1k9w4v7d2n8x3c5m0q6z8r2t4y6u8p0s2a4d6f8h0", index: 2 },
  dust: "0",
  nextPayout: "sent",
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
  const active = world.launch.network;
  return { chosen: saved.chosen, saved: saved.saved, active, forcedByFlag: world.networkForced ?? forced(active.kind) };
}

const networkInfo = (choice: ChosenNetwork): NetworkInfo => ({ kind: choice, dirName: choice, unitName: choice === "mainnet" ? "DASH" : "tDASH" });

/** The other networks' worlds of this data root (row 74): a switch keeps the world it leaves, as
 * the engine keeps that network's directory, and opens the other one's, or a new one with no
 * wallet. The page boots again after a switch, so the worlds are kept in the tab's session
 * storage until the next page takes them (`switchedWorld`). */
const SWITCH_KEY = "dw-fixture-switch";
let otherNetworks: Record<string, World> = {};

/** The world a switch left for this page load, if this load follows one; consumed. */
export function switchedWorld(): World | null {
  if (typeof sessionStorage === "undefined") return null;
  const saved = sessionStorage.getItem(SWITCH_KEY);
  if (saved === null) return null;
  sessionStorage.removeItem(SWITCH_KEY);
  const { active, others } = JSON.parse(saved) as { active: World; others: Record<string, World> };
  otherNetworks = others;
  return active;
}

/** What the gate waits for (`restore_status`), as the engine counts it: while RESTORED is set with
 * Platform off, Platform's first check is not waited for; `platformOff` is the session's. */
function restoreStatusOf(world: World): RestoreStatus {
  const s = world.restoreStatus ?? { waiting: [], historyWallets: [], clearFailed: false, platformOnly: false, platformOff: false };
  const off = world.noPlatform === true;
  return { ...s, waiting: off && !s.platformOnly ? s.waiting.filter((w) => w !== "platformPass") : s.waiting, platformOff: off };
}

/** Clears RESTORED once nothing holds it; with Platform off, Platform's check stays held (C6). */
function settleRestored(world: World) {
  const s = restoreStatusOf(world);
  if (s.waiting.length === 0 && s.historyWallets.length === 0 && !s.clearFailed) {
    world.restored = false;
    world.restoreStatus = world.noPlatform ? { waiting: ["platformPass"], historyWallets: [], clearFailed: false, platformOnly: true, platformOff: true } : undefined;
  }
}

function keepWorlds(world: World) {
  if (typeof sessionStorage !== "undefined") sessionStorage.setItem(SWITCH_KEY, JSON.stringify({ active: world, others: otherNetworks }));
}

/** Closes the window's network as the engine does (grants, leases and mixing gone, the vault
 * locked) and opens `to`: its kept world, or a new one with no wallet. The choice and the display
 * settings are the data root's, shared by both. */
function moveWorld(world: World, to: ChosenNetwork) {
  const left = restarted(JSON.parse(JSON.stringify(world)) as World);
  otherNetworks[left.launch.network.dirName] = left;
  const kept = otherNetworks[to];
  delete otherNetworks[to];
  const next = kept ?? buildWorld("no-wallet", left.launch.theme);
  next.launch = { ...left.launch, network: networkInfo(to) };
  next.settings = left.settings;
  next.networkSwitch = left.networkSwitch;
  for (const key of Object.keys(world)) delete (world as unknown as Record<string, unknown>)[key];
  Object.assign(world, next);
}

const sum = (addresses: PlatformWorld["addresses"]) => addresses.reduce((n, a) => n + BigInt(a.credits), 0n);
const total = (p: PlatformWorld) => sum(p.addresses);

/** A held top-up from Platform whose inputs the fixture knows: a move is its exit (C3-9). */
function heldTopUp(p: PlatformWorld): PlatformActivity | null {
  const held = p.activity.find((a) => a.kind === "toIdentity" && isHeldOp(a));
  return held && p.heldInputs?.[held.id] ? held : null;
}

/** A held send (failed by height, may still land): "Move to Dash wallet" at its inputs is its exit. */
const heldSendOf = (p: PlatformWorld): PlatformActivity | null => p.activity.find(isHeldSend) ?? null;

/** The oldest held cancel Platform answered, as the engine's quote picks it: "Move to Dash wallet" at its inputs
 * is its exit, beside its re-sends. */
const heldCancelOf = (p: PlatformWorld): PlatformActivity | null => p.activity.filter(isHeldCancel).at(-1) ?? null;

/** A held move or contact payment Platform answered that is too small to cancel and has no cancel:
 * "Move to Dash wallet" at its inputs is its exit. */
const tooSmallOf = (p: PlatformWorld): PlatformActivity | null =>
  p.activity.find((a) => (a.kind === "moveToCore" || a.kind === "contactPayment") && a.state === "failed" && a.mayLand && BigInt(a.credits) < CANCEL_MIN_CREDITS && !p.activity.some((c) => c.cancels === a.id && isHeldOp(c))) ?? null;

/** What a move is the named exit of, as the engine's quote names it: a held top-up, send or cancel, or a
 * held move or payment too small to cancel. */
const namedExitOf = (p: PlatformWorld): PlatformActivity | null => heldTopUp(p) ?? heldSendOf(p) ?? heldCancelOf(p) ?? tooSmallOf(p);

/** Whether `a` is the held cancel `cancel` sent again on its own terms: of the same item, the same amount, fee
 * and address (the engine's `resends`). */
const resendOf = (cancel: PlatformActivity, a: PlatformActivity): boolean =>
  a.id !== cancel.id && a.kind === "send" && !!cancel.cancels && a.cancels === cancel.cancels && a.credits === cancel.credits && a.feeCredits === cancel.feeCredits && a.to === cancel.to;

/** The inputs a send signed: where the world names them, else its fee source (the fixture's quote plans
 * one input), else every address (a Max). */
function sendInputs(p: PlatformWorld, send: PlatformActivity): string[] {
  const source = p.terms[send.id]?.quote.feeSource;
  return p.heldInputs?.[send.id] ?? (source ? [source] : p.addresses.map((a) => a.address));
}

/** The inputs of a held send, move, contact payment or top-up from Platform, where the world names them:
 * its exit (C3-9) moves those only. `null`: no such hold. */
function exitInputs(p: PlatformWorld): string[] | null {
  const send = heldSendOf(p);
  if (send) return sendInputs(p, send);
  const held = heldExitOp(p.activity);
  return held ? (p.heldInputs?.[held.id] ?? null) : null;
}

/** The addresses a move takes from: a held top-up's inputs, or all of them. */
function moveSources(p: PlatformWorld) {
  const only = exitInputs(p);
  return only ? p.addresses.filter((a) => only.includes(a.address)) : p.addresses;
}

/** What a move of everything would take: nothing when it would be refused (it does not clear the fee). */
const movable = (p: PlatformWorld) => (total(p) > MOVE_FEE_CREDITS ? total(p) : 0n);

/** As the engine's `address_sends_open`: this version keeps the addresses' funds paths off on mainnet. */
const sendsOff = (world: Ctx["world"]) => world.launch.network.kind === "mainnet";

function refuseSendsOff(world: Ctx["world"]) {
  if (sendsOff(world)) throw fail("platform.feature_off", PLATFORM_ADDRESS_SENDS, { kind: "feature", feature: PLATFORM_ADDRESS_SENDS });
}

function balance(world: Ctx["world"], p: PlatformWorld): PlatformAddressBalance {
  const credits = total(p) + BigInt(p.dust);
  const off = sendsOff(world);
  return {
    credits: String(credits),
    movableCredits: String(movable(p)),
    duffs: Number(movable(p) / CREDITS_PER_DUFF),
    addresses: [...p.addresses].sort((a, b) => Number(BigInt(b.credits) - BigInt(a.credits))),
    syncedAt: p.syncedAt,
    syncing: false,
    sendsActive: !off && (p.sendsActive ?? true),
    sendsOff: off ? PLATFORM_ADDRESS_SENDS : null,
  };
}

/** The platform state of `world`, or the absent-command rejection. */
function platformOf(world: Ctx["world"], call: string): PlatformWorld {
  if (world.pending.platform === null) throw absent(call);
  return world.pending.platform;
}

/** `contact_pay_quote` from the Platform balance: the engine's refusals in its order (a held move
 * or send, the withdrawal range, the balance), then the amount, about what arrives, the fee, and a
 * grant for the amount and three fee estimates (P8). */
export function contactPayoutQuote(world: Ctx["world"], walletId: string, credits: string): ContactPayQuoteInfo {
  wallet(world, walletId);
  const p = world.pending.platform;
  if (p === null) throw fail("platform.feature_off", "platform_addresses", { kind: "feature", feature: "platform_addresses" });
  const held = p.activity.filter(isHeldOp);
  if (held.length > 0) throw moveHeld(held[0].id);
  const c = BigInt(credits);
  if (c < PAYOUT_MIN_CREDITS || c > PAYOUT_MAX_CREDITS)
    throw fail("credits.withdrawal_out_of_range", "out of range", { kind: "creditRange", min: String(PAYOUT_MIN_CREDITS), max: String(PAYOUT_MAX_CREDITS) });
  const most = c + 3n * PAYOUT_FEE_CREDITS;
  if (most > total(p)) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(most), available: String(total(p)) });
  const expectedDuffs = Math.max(0, Number(c / CREDITS_PER_DUFF) - UNLOCK_FEE_DUFFS);
  return { source: "platform", credits: String(c), expectedDuffs, feeCredits: String(PAYOUT_FEE_CREDITS), grant: { maxDuffs: 0, maxCredits: String(most) } };
}

/** `contact_pay` from the Platform balance under `grant`: as `nextPayout` says, sent (the debit and
 * a `contactPayment` row), unknown (a pending row that holds the balance) or not sent (nothing). */
export function contactPayout(world: Ctx["world"], walletId: string, credits: string, grant: string, contact: TxCounterparty): { reference: string; state: PayoutStateName } {
  const q = contactPayoutQuote(world, walletId, credits);
  const g = checkGrant(world, grant, walletId, ["platformOp"]);
  if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
  if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.grant.maxCredits))
    throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.grant.maxCredits, remaining: g.grant.purpose.maxCredits });
  useGrant(world, grant);
  const p = platformOf(world, "contact_pay");
  const state = p.nextPayout;
  const reference = `move-${p.activity.length + 1}`;
  if (state === "notSent") return { reference, state };
  const sent = state === "sent";
  const debit = BigInt(q.credits) + PAYOUT_FEE_CREDITS;
  p.activity = [
    {
      id: reference,
      kind: "contactPayment",
      state: sent ? "done" : "pending",
      credits: sent ? String(debit) : q.credits,
      address: null,
      to: null,
      expectedDuffs: q.expectedDuffs,
      feeCredits: sent ? q.feeCredits : null,
      at: Math.floor(FIXTURE_NOW.getTime() / 1000),
      mayLand: !sent,
      estimated: false,
      contact,
    },
    ...p.activity,
  ];
  if (sent) spend(p, debit);
  return { reference, state };
}

/** The credits a move of `amount` takes: all of them, or that many (the quote's refusals). */
function moveCredits(p: PlatformWorld, amount: WithdrawAmount): bigint {
  const available = sum(moveSources(p));
  // An amount is the withdrawal, as the engine's: the move also keeps its fee.
  const credits = amount.kind === "all" ? available : BigInt(amount.credits) + MOVE_FEE_CREDITS;
  if (available === 0n || credits <= MOVE_FEE_CREDITS) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(MOVE_FEE_CREDITS + 1n), available: String(available) });
  if (credits > available) throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(credits), available: String(available) });
  return credits;
}

function quote(ctx: Ctx, walletId: string, amount: WithdrawAmount): MoveToCoreQuote {
  wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, "platform_move_to_core_quote");
  // On mainnet only a held move's exit passes.
  if (!p.activity.some(isHeldOp)) refuseSendsOff(ctx.world);
  // As the engine's quote: a send or cancel Platform has not answered holds the next move, refused before
  // any passphrase. A held send's or cancel's exit is this move.
  const sending = p.activity.find((a) => a.kind === "send" && a.state === "pending");
  if (sending) throw moveHeld(sending.id);
  // Any other held cancel than the named one and its identical re-sends holds it too.
  const named = namedExitOf(p);
  const otherCancel = p.activity.find((a) => !!a.cancels && isHeldOp(a) && a.id !== named?.id && !(named && resendOf(named, a)));
  if (otherCancel) throw moveHeld(otherCancel.id);
  const credits = moveCredits(p, amount);
  // An exit signs exactly its held record's inputs: an amount the largest of fewer of them carry is refused
  // (on mainnet, the flag's refusal).
  const held = heldTopUp(p) ?? heldExitOp(p.activity);
  if (held && amount.kind === "credits") {
    const sources = moveSources(p).map((a) => BigInt(a.credits)).sort((a, b) => (a < b ? 1 : a > b ? -1 : 0));
    let carried = 0n;
    if (sources.slice(0, -1).some((c) => (carried += c) >= credits)) {
      refuseSendsOff(ctx.world);
      throw moveHeld(held.id);
    }
  }
  const to = HANDLERS.current_receive_address!({ walletId }, ctx).address;
  return { credits: String(credits), feeCredits: String(MOVE_FEE_CREDITS), expectedDuffs: Number((credits - MOVE_FEE_CREDITS) / CREDITS_PER_DUFF), to, grant: { maxDuffs: 0, maxCredits: String(credits) }, exitOf: namedExitOf(p)?.id ?? null };
}

/** `platform_cancel_quote`, as the engine's: the held move or contact payment `of`, answered, beside no
 * other hold that its document could not tell apart; its amount, to the wallet's own receive address. */
function cancelQuote(ctx: Ctx, walletId: string, of: string): PlatformCancelQuote {
  wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, "platform_cancel_quote");
  // On mainnet only a held cancel of `of` is quoted again; a new one is off.
  if (!p.activity.some((a) => a.cancels === of && isHeldOp(a))) refuseSendsOff(ctx.world);
  const held = p.activity.find((a) => a.id === of && (a.kind === "moveToCore" || a.kind === "contactPayment") && isHeldOp(a));
  if (!held) throw fail("credits.quote_changed", "that move or payment no longer holds");
  if (held.state === "pending") throw moveHeld(held.id);
  // Its own held cancel is quoted again on that cancel's terms.
  const resend = p.activity.find((a) => a.cancels === of && isHeldOp(a) && a.state !== "pending");
  const other = p.activity.find((a) => a.id !== of && a !== resend && isHeldOp(a) && (a.state === "pending" || !!a.cancels || (a.kind !== "moveToCore" && a.kind !== "contactPayment")));
  if (other) throw moveHeld(other.id);
  if (total(p) < BigInt(held.credits)) throw fail("credits.quote_changed", "the held inputs no longer carry its amount");
  if (resend) return { of, credits: resend.credits, feeCredits: resend.feeCredits ?? String(SEND_FEE_CREDITS), to: resend.to ?? p.receive.address, grant: { maxDuffs: 0, maxCredits: resend.credits } };
  if (BigInt(held.credits) < CANCEL_MIN_CREDITS) throw fail("credits.send_out_of_range", "below the minimum", { kind: "creditMinimum", min: String(SEND_MIN_CREDITS) });
  return { of, credits: held.credits, feeCredits: String(SEND_FEE_CREDITS), to: p.receive.address, grant: { maxDuffs: 0, maxCredits: held.credits } };
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
  // On mainnet only a held send's exact retry passes.
  if (sendsOff(ctx.world) && !p.activity.some((a) => a.id === retryOf && a.kind === "send" && isHeldOp(a) && retriedBy(p.terms[a.id], to.trim(), amount))) refuseSendsOff(ctx.world);
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

const NOT_BLOCKING: readonly PendingPlatformFund["state"][] = ["checking", "tooSmall", "unsupported"];

function fundQuote(ctx: Ctx, walletId: string, duffs: number, call: string): PlatformFundQuote {
  const w = wallet(ctx.world, walletId);
  const p = platformOf(ctx.world, call);
  refuseSendsOff(ctx.world);
  if (duffs < FUND_MIN_DUFFS) throw fail("credits.below_minimum", "below the minimum", { kind: "limit", duffs: FUND_MIN_DUFFS });
  // As the engine: a lock that is checking, too small or unsupported blocks no new add.
  const blocking = p.funds.find((f) => !NOT_BLOCKING.includes(f.state));
  if (blocking) throw fail("credits.fund_pending", "an earlier add is pending", { kind: "txid", txid: blocking.txid });
  const totalDuffs = duffs + FUND_LOCK_FEE_DUFFS;
  const available = w.coins.filter((c) => c.spendable && !c.reserved && !c.userLocked && !c.coinjoinDenominated && c.coinjoinRounds === null).reduce((n, c) => n + c.amount, 0);
  if (totalDuffs > available) throw fail("credits.funding_insufficient", "insufficient funds", { kind: "duffs", needed: totalDuffs, available });
  const inputs = lockInputs(w, totalDuffs);
  if (inputs > SHIELD_LOCK_INPUTS) throw fail("credits.too_many_inputs", "too many inputs", { kind: "number", value: inputs });
  return { duffs, feeDuffs: FUND_LOCK_FEE_DUFFS, totalDuffs, feeCredits: String(FUND_FEE_CREDITS), credits: String(fundCredits(duffs)), to: p.receive.address, grant: { maxDuffs: totalDuffs, maxCredits: "0" }, max: null };
}

/** A fund's quote of `amount`: an amount with the fee on top, or a Core Max (`core_max.rs`). */
function fundQuoteOf(ctx: Ctx, walletId: string, amount: FundAmount, call: string): PlatformFundQuote {
  if (amount.kind === "duffs") return fundQuote(ctx, walletId, amount.duffs, call);
  const p = platformOf(ctx.world, call);
  const blocking = p.funds.find((f) => !NOT_BLOCKING.includes(f.state));
  if (blocking) throw fail("credits.fund_pending", "an earlier add is pending", { kind: "txid", txid: blocking.txid });
  const plan = coreMaxPlan(ctx.world, walletId, "credits", FUND_MIN_DUFFS);
  return { duffs: plan.duffs, feeDuffs: plan.feeDuffs, totalDuffs: plan.totalDuffs, feeCredits: String(FUND_FEE_CREDITS), credits: String(fundCredits(plan.duffs)), to: p.receive.address, grant: { maxDuffs: plan.totalDuffs, maxCredits: "0" }, max: plan.max };
}

/** As the engine's `refuse_on_mainnet`: a payment from a balance is off on mainnet, before anything else. */
function refusePayoutOnMainnet(world: Ctx["world"], source: PaymentSourceName) {
  if (source === "platform" && world.launch.network.kind === "mainnet") throw fail("platform.feature_off", "platform_contact_payment", { kind: "feature", feature: "platform_contact_payment" });
}

export const PENDING_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  recover_in_fresh_folder: ({ network }: CommandArgs["recover_in_fresh_folder"], { world }: Ctx) => {
    const refused = network == null ? world.openRefusal !== undefined : world.networkSwitch?.damaged === network;
    if (!refused) throw fail("invalid_argument", "the last open of the network was not refused", { kind: "reason", reason: "not refused" });
    if (network != null) {
      moveWorld(world, network);
      world.pending.networkChoice = { chosen: network, saved: true };
      delete world.networkSwitch?.damaged;
    }
    (world.restoreCalls ??= []).push("recover");
    startOverInNewFolder(world);
    return null;
  },
  // App::quit_app and restart_app, from the screens shown instead of the wallet. A restart frees a
  // store the session before held, and the page starts again.
  quit_app: (_: Record<string, never>, { world }: Ctx) => {
    (world.appExits ??= []).push("quit");
    return null;
  },
  restart_app: (_: Record<string, never>, { world, relaunch }: Ctx) => {
    (world.appExits ??= []).push("restart");
    world.storageHeld = undefined;
    // The start's drain finishes once the other program has let go, or after a passing failure.
    const shielded = world.pending.shielded;
    if (shielded && (shielded.closed === "in_use" || shielded.closed === "failed")) shielded.closed = undefined;
    if (world.networkSwitch) world.networkSwitch.reopenFails = false;
    world.noPlatform = undefined;
    world.restoreRefusedNetwork = undefined;
    // The restore asked for runs at the start, before the stores are checked: the earlier data opens.
    if (world.restoreRequested) {
      world.restoreRequested = false;
      world.openRefusal = undefined;
      world.restored = true;
    }
    relaunch?.();
    return null;
  },
  // App::restore_available, request_restore, keep_current_files and cancel_restore (DTO contract v3),
  // for the window's network, before it opens.
  restore_available: (_: { network: string | null }, { world }: Ctx) => {
    (world.restoreCalls ??= []).push("restoreAvailable");
    return world.restoreAvailable === true;
  },
  request_restore: ({ network }: { network: string | null }, { world }: Ctx) => {
    (world.restoreCalls ??= []).push("requestRestore");
    const target = network ?? world.launch.network.dirName;
    if (target !== world.restoreRefusedNetwork) throw fail("invalid_argument", "the last open of this network was not refused with Restore earlier data");
    if (!world.restoreAvailable) throw storeRestoreError(restoreFailure("refused:missingBackup", `${FIXTURE_DATA_DIR}/${target}`));
    world.restoreRequested = true;
    return null;
  },
  // Keep runs KeepLive: the request and the journal go, RESTORED stays.
  keep_current_files: (_: { network: string | null }, { world }: Ctx) => {
    (world.restoreCalls ??= []).push("keepCurrentFiles");
    if (world.openRestore?.state === "foreign") {
      world.openRestore = undefined;
      world.restored = true;
    }
    return null;
  },
  // Only the request goes; nothing was changed, so the current data opens.
  cancel_restore: (_: { network: string | null }, { world }: Ctx) => {
    (world.restoreCalls ??= []).push("cancelRestore");
    if (world.openRestore?.state === "notStarted") world.openRestore = undefined;
    return null;
  },
  // B addendum 1: what RESTORED waits for; Try again, which writes a clear that failed before; and
  // Finish without Dash Platform, which closes the network unless busy and opens it without Platform
  // (as usual if that fails), under one lock as `switch_network` does.
  restore_status: (_: Record<string, never>, { world }: Ctx) => (world.restored || world.restoreStatus?.platformOnly ? restoreStatusOf(world) : null),
  restore_retry: (_: Record<string, never>, { world }: Ctx) => {
    (world.restoreCalls ??= []).push("restoreRetry");
    if (world.restored && world.restoreStatus?.clearFailed) {
      world.restoreStatus = { ...world.restoreStatus, clearFailed: false };
      settleRestored(world);
    }
    return null;
  },
  finish_without_platform: (_: Record<string, never>, { world }: Ctx) => {
    // Nothing unless Platform's first check is what RESTORED waits for.
    if (!world.restored || !restoreStatusOf(world).waiting.includes("platformPass")) return openedNetwork(world, restoredNetwork(world));
    const knob = world.finishWithoutPlatform;
    if (knob === "busy" || knob === "draining")
      throw fail("app.network_busy", "closing the network would cut what runs", {
        kind: "networkBusy",
        flows: knob === "busy" ? ["registration"] : [],
        mixing: false,
        broadcast: false,
        stalled: false,
        draining: knob === "draining",
      });
    restarted(world);
    if (knob === "rolledBack") {
      world.finishWithoutPlatform = undefined;
      throw fail("app.network_switch_failed", `${world.launch.network.dirName} did not open without Platform: fixture`, {
        kind: "networkSwitch",
        network: world.launch.network,
        rolledBack: true,
      });
    }
    world.noPlatform = true;
    settleRestored(world);
    keepWorlds(world);
    return openedNetwork(world, restoredNetwork(world));
  },
  // DashPay::contact_pay_quote and contact_pay (C-funding D-C5): the payee as a draft checks it,
  // then the source: Shielded is off until its slice; Platform pays one address withdrawal.
  contact_pay_quote: ({ walletId, identity, contact, credits, source }: CommandArgs["contact_pay_quote"], { world }: Ctx) => {
    refusePayoutOnMainnet(world, source);
    payeeOfPayout(world, walletId, { identity, contact }, credits, source);
    return contactPayoutQuote(world, walletId, credits);
  },
  contact_pay: ({ walletId, identity, contact, payment, grant }: CommandArgs["contact_pay"], ctx: Ctx) => {
    const pair = { identity, contact };
    refusePayoutOnMainnet(ctx.world, payment.source);
    if (grant === "") throw fail("invalid_argument", "empty grant");
    const c = payeeOfPayout(ctx.world, walletId, pair, payment.credits, payment.source);
    const counterparty = { identity: contact, username: c.summary.username, displayName: c.summary.displayName, avatarUrl: c.profile?.avatarUrl ?? null };
    const out = contactPayout(ctx.world, walletId, payment.credits, grant, counterparty);
    payoutOnContact(ctx, walletId, pair, c, out, payment.credits);
    const p = ctx.world.pending.platform;
    if (p?.loseReply) {
      p.loseReply = false;
      throw fail("internal", "the reply was lost after the payout was recorded");
    }
    return out;
  },
  get_network_choice: (_: unknown, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent("get_network_choice");
    return networkView(world);
  },
  // Row 74: dw-app's `switch_network`, with the refusals and failures a test sets in
  // `world.networkSwitch`.
  switch_network: ({ network }: { network: ChosenNetwork }, { world }: Ctx): NetworkSwitch => {
    if (world.pending.networkChoice === null) throw absent("switch_network");
    if (network !== "mainnet" && network !== "testnet") throw fail("invalid_argument", "not a network", { kind: "reason", reason: "not a network" });
    if (forced(world.launch.network.kind)) {
      world.pending.networkChoice = { chosen: network, saved: true };
      return { opened: null, choice: networkView(world) };
    }
    if (network !== world.launch.network.kind) {
      const knobs = world.networkSwitch ?? {};
      if (knobs.busy) throw fail("app.network_busy", "closing the network would cut what runs", { kind: "networkBusy", stalled: false, draining: false, ...knobs.busy });
      if (knobs.closeRefused) {
        const refusal = {
          wallet: ["wallet", "walletOpening"],
          shielded_spend_unresolved: ["shielded_spend_unresolved", "shieldedSpendUnresolved"],
          transfer_preparing: ["app.transfer_preparing", "transferPreparing"],
        } as const;
        const [code, reason] = refusal[knobs.closeRefused];
        throw fail(code, "the close was refused", { kind: "networkCloseRefused", reason });
      }
      if (knobs.fail) {
        // The close happened: what the page knew of this network is gone, so it boots again.
        restarted(world);
        world.networkSwitch = { ...knobs, fail: undefined, reopenFails: !knobs.fail.rolledBack };
        keepWorlds(world);
        throw fail("app.network_switch_failed", `fixture: opening ${network} failed`, { kind: "networkSwitch", network: networkInfo(network), rolledBack: knobs.fail.rolledBack });
      }
      if (knobs.damaged === network) {
        restarted(world);
        keepWorlds(world);
        throw storeRefusedError(STORE_SCREENS.damaged(`${FIXTURE_DATA_DIR}/${network}`));
      }
      const before = world.pending.networkChoice;
      moveWorld(world, network);
      // Saved last, as dw-app saves it; `saveFails` keeps the choice the switch started from.
      world.pending.networkChoice = knobs.saveFails ? before : { chosen: network, saved: true };
      keepWorlds(world);
      return { opened: openedNetwork(world, world.launch.network), choice: networkView(world) };
    }
    world.pending.networkChoice = { chosen: network, saved: true };
    return { opened: openedNetwork(world, world.launch.network), choice: networkView(world) };
  },
  ...SHIELDED_HANDLERS,
  ...RECOVERY_HANDLERS,
  ...REMAINDER_HANDLERS,
  platform_addresses: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return balance(world, platformOf(world, "platform_addresses"));
  },
  platform_activity: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    return platformOf(world, "platform_activity").activity;
  },
  platform_addresses_sync: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    const p = platformOf(world, "platform_addresses_sync");
    p.syncedAt = Math.floor(FIXTURE_NOW.getTime() / 1000);
    return balance(world, p);
  },
  platform_receive_address: ({ walletId }: { walletId: string }, { world }: Ctx) => {
    wallet(world, walletId);
    const p = platformOf(world, "platform_receive_address");
    refuseSendsOff(world);
    return p.receive;
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
    if (!op?.quote.retryOf) refuseSendsOff(world);
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
    preCheck(p);
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
  platform_fund_quote: ({ walletId, amount }: { walletId: string; amount: FundAmount }, ctx: Ctx) => fundQuoteOf(ctx, walletId, amount, "platform_fund_quote"),
  platform_fund: ({ walletId, amount, grant }: { walletId: string; amount: FundAmount; grant: string }, ctx: Ctx): PlatformFundOutcome => {
    const { world } = ctx;
    const q = fundQuoteOf(ctx, walletId, amount, "platform_fund");
    if (q.max && (amount.kind !== "quotedAll" || amount.coinSet !== q.max.coinSet)) throw fail("credits.coins_changed", "the coins changed since the quote");
    const duffs = q.duffs;
    const p = platformOf(world, "platform_fund");
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (g.grant.purpose.maxDuffs < q.totalDuffs)
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "funding", needed: String(q.totalDuffs), remaining: String(g.grant.purpose.maxDuffs) });
    if (q.max && g.grant.purpose.maxDuffs !== q.totalDuffs) throw fail("credits.coins_changed", "the coins changed since the quote");
    useGrant(world, grant);
    const txid = `f0${(p.activity.length + 1).toString(16).padStart(2, "0")}`.padEnd(64, "d");
    const pending = p.nextFund !== "done";
    const credits = BigInt(q.credits);
    p.activity = [{ id: `fund-${txid}`, kind: "fromCore", state: pending ? "pending" : "done", credits: q.credits, address: q.to, to: null, expectedDuffs: null, feeCredits: null, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: false, estimated: false }, ...p.activity];
    switch (p.nextFund) {
      case "done":
        applyFund(p, txid, credits);
        return { txid, state: { kind: "applied", credits: q.credits } };
      case "proofWaiting":
        p.funds = [...p.funds, { txid, duffs, proved: false, consumptionUnknown: false, state: "confirming" }];
        return { txid, state: { kind: "proofWaiting" } };
      case "checking":
        p.funds = [...p.funds, { txid, duffs, proved: true, consumptionUnknown: true, state: "checking" }];
        return { txid, state: { kind: "consumptionUnknown" } };
      case "notApplied":
        p.funds = [...p.funds, { txid, duffs, proved: true, consumptionUnknown: false, state: "ready" }];
        return { txid, state: { kind: "notApplied", error: "platform.unavailable" } };
      case "approvalEnded":
        p.funds = [...p.funds, { txid, duffs, proved: true, consumptionUnknown: false, state: "ready" }];
        return { txid, state: { kind: "notApplied", error: "platform.lease_revoked" } };
    }
  },
  pending_platform_funds: ({ walletId }: { walletId: string }, { world }: Ctx): PendingPlatformFund[] => {
    wallet(world, walletId);
    return platformOf(world, "pending_platform_funds").funds;
  },
  resume_platform_fund: ({ walletId, txid, grant }: { walletId: string; txid: string; grant: string }, { world }: Ctx): PlatformFundOutcome => {
    wallet(world, walletId);
    const p = platformOf(world, "resume_platform_fund");
    const fund = p.funds.find((f) => f.txid === txid);
    if (!fund) throw fail("invalid_argument", "not a pending add", { kind: "reason", reason: "not a pending add" });
    if (fund.state === "tooSmall" || fund.state === "unsupported") throw fail("invalid_argument", "can't be finished in this version", { kind: "reason", reason: "can't be finished in this version" });
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs < 1) throw fail("platform.grant_invalid", "grant invalid");
    useGrant(world, grant);
    if (p.nextResume === "needsGrant") {
      p.nextResume = "done";
      throw fail("platform.needs_grant", "needs a grant", { kind: "needsGrant", purpose: "funding" });
    }
    const credits = fundCredits(fund.duffs);
    p.nextFund = "done";
    applyFund(p, txid, credits);
    return { txid, state: { kind: "applied", credits: String(credits) } };
  },
  platform_move_to_core_quote: ({ walletId, amount }: { walletId: string; amount: WithdrawAmount }, ctx: Ctx) => quote(ctx, walletId, amount),
  platform_move_to_core: ({ walletId, amount, to, exitOf, grant }: { walletId: string; amount: WithdrawAmount; to: string; exitOf: string | null; grant: string }, ctx: Ctx): MoveToCoreOutcome => {
    const { world } = ctx;
    // As the engine, first: the exit of a top-up, send, cancel or too small move or payment that no longer
    // holds is not the move Confirm showed.
    const exited = namedExitOf(platformOf(world, "platform_move_to_core"));
    if (exitOf != null && exited?.id !== exitOf) {
      refuseSendsOff(world);
      throw fail("credits.quote_changed", "the held top-up settled since this move was quoted");
    }
    // An exit moves its quoted amount: inputs that no longer carry it are not the move Confirm showed.
    const q = (() => {
      try {
        return quote(ctx, walletId, amount);
      } catch (e) {
        if (exitOf != null && (e as AppError).code === "platform.insufficient_credits") throw fail("credits.quote_changed", "the held inputs no longer carry the quoted amount");
        throw e;
      }
    })();
    // Only to one of the wallet's own receiving addresses (the quote's).
    if (to !== q.to) throw fail("invalid_argument", "not an address of this wallet", { kind: "reason", reason: "not an address of this wallet" });
    const p = platformOf(world, "platform_move_to_core");
    // The named cancel's identical re-sends that hold beside it: its exit runs beside them and settles them with it.
    const resends = exitOf != null && exited ? p.activity.filter((a) => isHeldOp(a) && resendOf(exited, a)).map((a) => a.id) : [];
    // A send that may still land, or a move Platform has not answered, holds the next move (it waits until it
    // settles). A move that failed by height does not: the next one signs at the same inputs and frees it.
    const holding = p.activity.find((a) => isHeldOp(a) && ((a.kind === "send" && a.id !== exitOf && !resends.includes(a.id)) || a.state === "pending"));
    if (holding) throw moveHeld(holding.id);
    preCheck(p);
    // The move or payment a held cancel cancels, which the exit's document settles with it.
    const cancelled = exitOf != null ? (exited?.cancels ?? null) : null;
    // A held move or payment too small to cancel that the move is the exit of: their documents say which went through.
    const direct = exitOf != null && (exited?.kind === "moveToCore" || exited?.kind === "contactPayment") ? exited.id : null;
    const supersedes = p.activity.filter((a) => a.kind === "moveToCore" && isHeldOp(a) && a.id !== cancelled && a.id !== direct).map((a) => a.id);
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    useGrant(world, grant);
    if (p.nextMove === "refused") throw fail("platform.unavailable", "the fixture refuses the move");
    if (p.nextMove === "tooManyCoins") throw fail("invalid_argument", "more than 50 separate balances", { kind: "reason", reason: "Your Platform addresses hold too many separate balances to move at once. Consolidate coins first" });
    // `lost`: the held top-up it exits executed in its place, so the move fails and moves nothing.
    const state = p.nextMove === "pending" ? "pending" : p.nextMove === "lost" || p.nextMove === "lostToCancel" ? "failed" : "done";
    const expectedDuffs = p.nextMove === "unproved" ? null : q.expectedDuffs;
    // As the engine, the exit of a held send or cancel carries its link in its id.
    const id = exitOf != null && exited?.kind === "send" ? `exit-of-${exited.id}-at-1-${p.activity.length + 1}` : `pm-${p.activity.length + 1}`;
    // The named cancel and its identical re-sends: of them the newest carries a cancel's win, the others
    // replaced beside a move and failed beside a payment.
    const cancels = cancelled !== null && exited ? [exited.id, ...resends] : [];
    const newestCancel = p.activity.filter((a) => cancels.includes(a.id)).reduce<PlatformActivity | null>((n, a) => (n && n.at >= a.at ? n : a), null)?.id;
    const ofPayment = p.activity.find((a) => a.id === cancelled)?.kind === "contactPayment";
    // Beside a held cancel, which of the three went through: the move, the cancel, or what it cancels. Beside a
    // held item too small to cancel, the move or that item.
    const winner = state === "done" ? id : state === "failed" ? (p.nextMove === "lostToCancel" ? newestCancel : (cancelled ?? direct)) : null;
    const feeCredits = state === "done" ? q.feeCredits : null;
    const exit = exitInputs(p);
    const lost = state === "failed" ? heldTopUp(p)?.id : undefined;
    // The exit of a held send: its document says which of the two went through; neither is replaced.
    const send = exitOf != null && exited?.kind === "send" ? exited.id : null;
    // Of a group of moves at the same inputs at most one executes: the new one replaces the held ones.
    p.activity = [
      { id, kind: "moveToCore", state, credits: q.credits, address: null, to: q.to, expectedDuffs, feeCredits, at: Math.floor(FIXTURE_NOW.getTime() / 1000), mayLand: state === "pending", estimated: false },
      // A held top-up and its exit: at most one executes. Once the exit is done (C3-9) the top-up can no
      // longer; while the exit is pending either may still.
      ...p.activity.map((a) =>
        supersedes.includes(a.id)
          ? { ...a, state: "replaced" as const, mayLand: false }
          : (cancelled !== null || direct !== null) && winner && (a.id === send || a.id === cancelled || a.id === direct || resends.includes(a.id))
            ? { ...a, state: a.id === winner ? ("done" as const) : !ofPayment && cancels.includes(a.id) && cancels.includes(winner) ? ("replaced" as const) : ("failed" as const), mayLand: false }
            : state === "done" && (a.kind === "toIdentity" || a.id === send) && isHeldOp(a)
            ? { ...a, state: "failed" as const, mayLand: false }
            : state === "failed" && a.kind === "toIdentity" && isHeldOp(a)
              ? { ...a, state: "done" as const, mayLand: false }
              : a,
      ),
    ];
    // A move that is `done` has emptied the addresses it took from; a lost exit's top-up landed as a done
    // one does; a `pending` one has not been seen yet.
    if (state === "done") p.addresses = exit ? p.addresses.filter((a) => !exit.includes(a.address)) : [];
    if (lost) landHeldTopUp(ctx, lost);
    // A lost exit of a send: the send went through, its recipient paid. Of a cancel: the cancel went
    // through (the amount less its fee back on its own address), or what it cancels did. Of an item too
    // small to cancel: that item went through.
    if (state === "failed" && (cancelled !== null || direct !== null) && exited) {
      spend(p, BigInt(exited.credits));
      if (winner === exited.id && cancelled !== null && exited.to) p.addresses = [...p.addresses, { address: exited.to, credits: String(BigInt(exited.credits) - SEND_FEE_CREDITS) }];
    } else if (state === "failed" && send) landSend(p, send);
    return { id, credits: q.credits, expectedDuffs, to: q.to, state, feeCredits };
  },
  platform_cancel_quote: ({ walletId, of }: { walletId: string; of: string }, ctx: Ctx) => cancelQuote(ctx, walletId, of),
  // As the engine: the cancel signs the held item's inputs at its nonces to the quote's own address. As
  // `nextMove` says, it goes through (the held item fails, the amount less the fee arrives at that
  // address), stays unanswered, or loses (`lost`: the held item went through, the cancel failed unpaid).
  platform_cancel: ({ walletId, of, to, grant }: { walletId: string; of: string; to: string; grant: string }, ctx: Ctx): PlatformSendOutcome => {
    const { world } = ctx;
    const q = cancelQuote(ctx, walletId, of);
    if (to !== q.to) throw fail("invalid_argument", "not an address of this wallet", { kind: "reason", reason: "not an address of this wallet" });
    const p = platformOf(world, "platform_cancel");
    preCheck(p);
    preCheck(p, p.cancelPreCheck);
    const g = checkGrant(world, grant, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < BigInt(q.credits))
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: q.credits, remaining: g.grant.purpose.maxCredits });
    useGrant(world, grant);
    if (p.nextMove === "refused") throw fail("platform.unavailable", "the fixture refuses the cancel");
    const state = p.nextMove === "pending" ? "pending" : p.nextMove === "lost" ? "failed" : "done";
    const at = Math.floor(FIXTURE_NOW.getTime() / 1000);
    const id = `pc-${p.activity.length + 1}`;
    const credits = BigInt(q.credits);
    const arrives = credits - SEND_FEE_CREDITS;
    const row: PlatformActivity = { id, kind: "send", state, credits: q.credits, address: null, to: q.to, expectedDuffs: null, feeCredits: state === "done" ? q.feeCredits : null, at, mayLand: state === "pending", estimated: false, cancels: of };
    // A held cancel of the same item is the one sent again: once the re-send settles, it no longer holds
    // (replaced by a re-send that went through beside a move, failed beside a payment).
    const earlier = (a: PlatformActivity) => a.cancels === of && isHeldOp(a) && state !== "pending";
    const replaced = state === "done" && p.activity.find((a) => a.id === of)?.kind !== "contactPayment";
    p.activity = [
      row,
      ...p.activity.map((a) =>
        earlier(a)
          ? { ...a, state: replaced ? ("replaced" as const) : ("failed" as const), mayLand: false }
          : a.id !== of
            ? a
            : state === "done"
              ? { ...a, state: "failed" as const, mayLand: false }
              : state === "failed"
                ? { ...a, state: "done" as const, mayLand: false }
                : a,
      ),
    ];
    if (state !== "pending") spend(p, credits);
    if (state === "done") {
      p.addresses = [...p.addresses, { address: q.to, credits: String(arrives) }];
      p.activity = [{ id: `pa-${p.activity.length + 1}`, kind: "incoming", state: "done", credits: String(arrives), address: q.to, to: null, expectedDuffs: null, feeCredits: null, at, mayLand: false, estimated: false }, ...p.activity];
    }
    return { id, credits: q.credits, amountCredits: q.credits, feeFromAmount: true, to: q.to, state, mayLand: state === "pending", feeCredits: row.feeCredits, estimated: false };
  },
};
