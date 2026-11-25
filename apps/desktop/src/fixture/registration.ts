// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Username registration in the fixture (DP1-09): the engine's DP1-02 flow and DP1-03 names as
// the wizard reaches them. A start writes a funded row and broadcasts one asset lock
// (`DashPayWorld.assetLocks` counts them, so a test proves a resume sends none); the run then
// moves one step per `advance`, which a test calls where the network would answer. A run lives in
// the session (`DashPayWorld.runs`) under its lease: a relaunch (`restarted`) drops it with every
// grant and lease, and a lock or passphrase change revokes the lease and parks it; the row then
// waits for the user again (`unlock` or `authorize`) until `resume_registration`.
// An invitation's claim (DP5-02, DP5-03 D17) is a row of its own kind: no asset lock and no duffs,
// its identity the claim's answer (the link wiped then), and the request to the inviter at the
// end, refused when `DashPayWorld.requestRefusal` says so. `World.invitations` sets a stashed
// invitation's status; one not set there is valid, from Bob.
// A start from the Platform balance (C-funding F4) stores its plan (`DashPayWorld.platformPlans`)
// and writes a row that commits nothing until its create; `DashPayWorld.platformCreates` says what
// each create comes to.
// A temporary name beside a contested label (D4 reversed) is priced with it and registered right
// after its contest starts, unless `DashPayWorld.temporaryRefused` (the engine only logs that).
// Not modelled: profiles (F2), the funding's coin selection and its debit
// from the balance, InstantSend timeouts, and proofs (a resumed `fundingSent` row waits for its
// InstantSend lock again).

import type { AppError, AppEvent } from "../api/generated/commands";
import type { FlowKindName } from "../api/generated/FlowKindName";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { InvitationStatus } from "../api/generated/InvitationStatus";
import type { Lease } from "../api/generated/Lease";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationPhaseName } from "../api/generated/RegistrationPhaseName";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationWaitName } from "../api/generated/RegistrationWaitName";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import type { UsernameRuleName } from "../api/generated/UsernameRuleName";
import { IDENTITY, type DashPayWorld } from "./dashpay";
import type { Ctx, Handlers } from "./handlers";
import { finishRestored, reportOf } from "./recovery";
import { isHeldOp } from "../platform/move";
import type { PlatformWorld } from "./pending";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
import type { ShieldedWorld } from "./shielded";
import { FIXTURE_NOW, secs } from "./wallet";

/** names.rs `MIN_LENGTH` and `MAX_LENGTH` (DP1-03's cap below DPNS's 63). */
const MIN_LENGTH = 3;
const MAX_LENGTH = 23;
/** The credits a name costs (the fixture quote's `credits`, contested or not). */
export const NAME_CREDITS = 2_000_000_000n;
/** How long the fixture's contests run: DPNS's two weeks on mainnet. */
const CONTEST_SECS = 14 * 24 * 3600;

/** The identity a fixture registration creates. */
export const NEW_IDENTITY = IDENTITY.alice;

/** The fixture invitation's inviter, as `invitation_status` proves one: DPNS name and profile. */
export const INVITER: InvitationInviter = { identity: IDENTITY.bob, username: "bob", displayName: "Bob Builder", avatarUrl: null };
/** What the fixture invitation funds (0.05 Dash: an uncontested claim, not a contested one). */
export const INVITATION_DUFFS = 5_000_001;
/** The credits a claim's identity and its request to the inviter cost (flows.rs's constants). */
const IDENTITY_CREDITS = 1_200_000_000n;
const REQUEST_CREDITS = 300_000_000n;
/** registration_rows.rs `Phase`'s order (`failed` is not a step). */
const PHASE_ORDER: RegistrationPhaseName[] = ["draft", "keysPrepared", "fundingSent", "proofWaiting", "identityRegistered", "nameRequested", "nameRegistered", "contested", "profileCreated", "done"];

/** A valid invitation from Bob, with `over` changed. */
export const validInvitation = (over: Partial<Extract<InvitationStatus, { kind: "valid" }>> = {}): InvitationStatus => ({
  kind: "valid",
  inviter: INVITER,
  fundingDuffs: INVITATION_DUFFS,
  contestedAllowed: false,
  expiresAt: null,
  ...over,
});

/** invitations.rs `invitation_status`: a link no longer stashed reads `expired`. */
export function invitationOf(world: World, linkId: string): InvitationStatus {
  if (!world.stash.some((s) => s.info.id === linkId && s.info.kind === "invitation")) return { kind: "expired" };
  return world.invitations?.[linkId] ?? validInvitation();
}

/** registration.rs `invitation_plan`: a claim's refusals, then its credits and the inviter it
 * asks (`requestInviter: false`, none: DP5-03 D7). */
export function claimPlan(world: World, dp: DashPayWorld, request: RegistrationRequest) {
  if (request.funding.kind !== "invitation") throw invalid("not a claim");
  if (dp.identities.length > 0) throw fail("invitation.already_has_identity", "the wallet has an identity");
  const status = invitationOf(world, request.funding.linkId);
  if (status.kind === "claimed") throw fail("invitation.claimed", "the invitation was claimed");
  if (status.kind === "expired") throw fail("invitation.expired", "the invitation is not stashed");
  if (status.kind === "invalid") throw fail("invitation.invalid", "the invitation is not valid");
  const check = validLabel(request.label);
  if (check.contested && !status.contestedAllowed) throw fail("name.unavailable_for_invite", "the invitation cannot pay for a contested name");
  const inviter = request.funding.requestInviter ? (status.inviter?.identity ?? null) : null;
  const credits = NAME_CREDITS + IDENTITY_CREDITS + (inviter ? REQUEST_CREDITS : 0n);
  return { status, inviter, credits, linkId: request.funding.linkId };
}

/** Names the fixture network already knows, by their homograph-safe form: taken, contested
 * and locked (DP1-03's `name_availability` answers). The wallet's directory counts as taken. */
export const NETWORK_NAMES: Record<string, NameAvailability> = {
  b0b: { kind: "taken", owner: IDENTITY.bob },
  "ma11ory": { kind: "taken", owner: IDENTITY.mallory },
  car01: { kind: "contestOpen", endsAt: secs(FIXTURE_NOW) + 5 * 24 * 3600, contenders: 2 },
  "sat0sh1": { kind: "locked" },
  // A plain name taken, for the instant username's form (DP1-09 D4 reversed).
  a11ce3: { kind: "taken", owner: IDENTITY.mallory },
};

/** dpp's `convert_to_homograph_safe_chars`: lower case, `o` → `0`, `i` and `l` → `1`. */
export const homographSafe = (label: string) => label.toLowerCase().replace(/o/g, "0").replace(/[il]/g, "1");

/** names.rs `username_check`. */
export function checkUsername(label: string): UsernameCheck {
  const length = [...label].length;
  const rules: [UsernameRuleName, boolean][] = [
    ["minLength", length >= MIN_LENGTH],
    ["maxLength", length <= MAX_LENGTH],
    ["allowedCharacters", /^[A-Za-z0-9-]*$/.test(label)],
    ["noEdgeHyphen", !label.startsWith("-") && !label.endsWith("-")],
    ["noDoubleHyphen", !label.includes("--")],
  ];
  const valid = rules.every(([, passed]) => passed);
  return {
    valid,
    normalized: homographSafe(label),
    contested: valid && /^[a-z01-]{3,19}$/.test(homographSafe(label)),
    rules: rules.map(([rule, passed]) => ({ rule, passed })),
  };
}

const fail = (code: AppError["code"], message: string, data: AppError["data"] = null): AppError => ({ code, message, data });

/** The engine refuses a Shielded-funded start until its flow step lands (m4 item 94). */
export const shieldedOff = (): AppError =>
  fail("platform.feature_off", "shielded_registration", { kind: "feature", feature: "shielded_registration" });
const platformOff = () => fail("platform.feature_off", "platform_registration", { kind: "feature", feature: "platform_registration" });
const invalid = (detail: string) => fail("invalid_argument", detail);

/** `name.invalid` with the rules `label` breaks, if it breaks one. */
function validLabel(label: string): UsernameCheck {
  const check = checkUsername(label);
  if (!check.valid) throw fail("name.invalid", `invalid username: ${label}`, { kind: "usernameRules", rules: check.rules.filter((r) => !r.passed).map((r) => r.rule) });
  return check;
}

function availability(dp: DashPayWorld, label: string): NameAvailability {
  const check = checkUsername(label);
  if (!check.valid) return { kind: "invalid", rules: check.rules.filter((r) => !r.passed).map((r) => r.rule) };
  const known = NETWORK_NAMES[check.normalized];
  if (known) return known;
  const owner = dp.directory.find((u) => homographSafe(u.username) === check.normalized);
  if (owner) return { kind: "taken", owner: owner.identity };
  return { kind: "available", contested: check.contested };
}

const finished = (r: RegistrationStatus) => r.phase === "done" || r.phase === "failed";

/** The live lease of the wallet's registration run. */
function runLease(world: World, walletId: string): Lease | undefined {
  return world.leases.find((l) => l.walletId === walletId && l.flow === "registration" && l.state.kind !== "ended" && l.state.kind !== "revoked");
}

/** `draft` has a run this session that can sign: a lock or a new credential (a passphrase
 * change) revokes its lease and parks it. A scenario's run without any lease counts as running. */
function active(world: World, walletId: string, dp: DashPayWorld, draft: string): boolean {
  if (!dp.runs.includes(draft)) return false;
  return runLease(world, walletId) !== undefined || !world.leases.some((l) => l.walletId === walletId && l.flow === "registration");
}

/** The row as `registrations` reports it: an unfinished row without an active run (none this
 * session, or one parked) says what it lacks first, as the engine's `idle_wait`: its funds' decision
 * (`needsFinish`, `fundingUnknown`), its lock's proof (`chainLock`: the fixture models none), then
 * the user's unlock or grant; its key went with the run (D21). */
export function reported(world: World, walletId: string, dp: DashPayWorld, r: RegistrationStatus): RegistrationStatus {
  // A copy: `advance` changes the world's row in place, and a read answering the same object
  // would look unchanged to the query cache. A parked run has ended, as the engine's does (D26).
  const running = active(world, walletId, dp, r.draft);
  if (r.phase === "done" || running) return { ...r, running };
  // A failed row that still owns an outstanding funding step (the engine's `Awaiting` standing)
  // keeps that step's wait; any other failed row waits for nothing (f1, Sol r1 P2).
  if (r.phase === "failed") return { ...r, waiting: awaitingWait(r.waiting), holdsKey: false, running: false };
  // A Platform-funded row's payment reads its create's record (C-funding F7, F22).
  const kept = r.waiting === "needsFinish" || r.waiting === "fundingUnknown" || (r.fromPlatform && (r.waiting === "paymentHeld" || r.waiting === "paymentUnknown"));
  // A parked run (its lease revoked or parked, D14) says what it parks for; an idle row its proof.
  const proofless = !dp.runs.includes(r.draft) && (r.phase === "fundingSent" || r.phase === "proofWaiting");
  const keyless = world.lock.state === "locked" || world.lock.state === "unlockedMixingOnly";
  const waiting: RegistrationWaitName = kept ? r.waiting! : proofless ? "chainLock" : keyless ? "unlock" : "authorize";
  return { ...r, waiting, holdsKey: false, running: false };
}

/** The waits registration.rs keeps on a failed `Awaiting` row: its funding's unknown outcome, or
 * its lock's proof (`chainLock`, `network`). */
const awaitingWait = (w: RegistrationStatus["waiting"]): RegistrationWaitName | null =>
  w === "fundingUnknown" || w === "chainLock" || w === "network" ? w : null;

/** What a running row waits for in `phase`: the funding's InstantSend lock, else nothing. */
const runningWait = (phase: RegistrationStatus["phase"]): RegistrationWaitName | null =>
  phase === "fundingSent" || phase === "proofWaiting" ? "instantSend" : null;

/** The engine after a relaunch: no grant, no lease, no run and no mixer survive, and an encrypted vault
 * opens locked. The rows and the asset-lock count are on disk, so they stay. */
export function restarted(world: World): World {
  world.grants = [];
  world.leases = [];
  world.flowLeases = {};
  if (world.lock.encrypted && world.lock.state !== "noVault") world.lock = { ...world.lock, state: "locked" };
  for (const dp of [world.dashpay, ...Object.values(world.dashpayOf ?? {})]) dp.runs = [];
  // Mixing is memory too: nothing resumes it (U-06 D5).
  world.coinjoin.mixers = {};
  return world;
}

export interface RegistrationDeps {
  wallet(w: World, walletId: string): FixtureWallet;
  dashpay(w: World, walletId: string, identity?: string): DashPayWorld;
  grant(w: World, id: string, walletId: string, kinds: ["platformOp"]): FixtureGrant | null;
  newLease(ctx: Ctx, walletId: string, flow: FlowKindName, grants: FixtureGrant[]): [string, Lease];
  setLease(ctx: Ctx, lease: Lease): Lease;
  writeLease(ctx: Ctx, walletId: string, grant: string, flows: FlowKindName[], quote: bigint): () => void;
}

/** The wallet's auto-selectable coins for a `CoreBalance` funding: its confirmed Dash outside
 * CoinJoin (the quote's `availableDuffs`, m4 item 52). */
export function fundable(w: FixtureWallet): number {
  const b = w.summary.balances;
  return b ? Math.max(0, b.confirmed - b.coinjoin) : 0;
}

/** PV13's smallest member, 0.03 Dash in credits: a plain name's Shielded registration (C-funding F14). */
export const SHIELDED_DENOMINATION = 3_000_000_000n;
/** registration_flow.rs `IDENTITY_CREATE_CREDITS`: the create's fee bound the quote takes from it. */
const CREATE_CREDITS = 300_000_000n;

/** registration.rs `shielded_readiness` over the fixture's Shielded world (C-funding F16, F17). */
export function shieldedReadiness(s: ShieldedWorld | null | undefined): ShieldedReadiness {
  const active = s?.active === true;
  const synced = active && s.ready && s.scanning !== true;
  const resting = s?.restingUntil !== undefined && s.restingUntil > secs(FIXTURE_NOW) ? s.restingUntil : null;
  const checks: [ShieldedCheckName, boolean][] = [
    ["poolActive", active],
    ["featureOn", true],
    ["denomination", active],
    ["balance", active && BigInt(s.balance) >= SHIELDED_DENOMINATION],
    ["noUnsettledSpend", active && s.unresolved.length === 0 && s.unrecorded.length === 0],
    ["synced", synced],
    ["rested", synced && resting === null],
  ];
  return { denomination: active ? String(SHIELDED_DENOMINATION) : null, checks: checks.map(([check, ready]) => ({ check, ready })), readyAt: resting };
}

/** A Shielded-funded quote (registration.rs `shielded_plan`): nothing in duffs; the grant covers
 * the denomination and the names. */
export function shieldedQuote(label: string, s: ShieldedWorld | null | undefined): RegistrationQuote {
  const { contested, credits } = costs(label);
  const readiness = shieldedReadiness(s);
  const denomination = readiness.denomination === null ? 0n : BigInt(readiness.denomination);
  const held = denomination > CREATE_CREDITS ? denomination - CREATE_CREDITS : 0n;
  return {
    contested,
    lockDuffs: 0,
    feeDuffs: 0,
    totalDuffs: 0,
    remainingCredits: String(held > credits ? held - credits : 0n),
    availableDuffs: 0,
    grant: { maxDuffs: 0, maxCredits: String(credits + denomination) },
    readiness,
    platform: null,
  };
}

/** dpp's minimum fee for an identity create from `inputs` addresses with six keys (2,000,000 +
 * 500,000 per input + 6 × 6,500,000), and its funding rules: the least an input claims, the least
 * the identity receives, and the most inputs one transition takes. */
const registerFee = (inputs: number) => 2_000_000n + 500_000n * BigInt(inputs) + 6n * 6_500_000n;
const MIN_INPUT = 100_000n;
const MIN_IDENTITY_FUNDING = 200_000n;
const MAX_INPUTS = 16;
/** registration_platform.rs `REGISTER_FEE_BAND`: input 0 keeps three fees. */
const FEE_BAND = 3n;

/** One input of a plan: the address and the credits it claims. */
type PlanInput = { address: string; claim: bigint };

/** A Platform-funded row's stored funding (C-funding F3): what the identity receives, what input 0
 * keeps for the fee, and each input's claim in address order, input 0 first. */
export interface PlatformRegisterPlan {
  funding: bigint;
  fee: bigint;
  reserve: bigint;
  inputs: PlanInput[];
}

/** `registered`: the create's proved result; `unknown`: no answer (F7); `held`: failed by height
 * and able to land still (F22); `paidFailure`: executed and failed in a block, and nothing
 * retries (F10, F12); `replanRefused`: a new plan no longer gives the quoted numbers, refused
 * before anything is signed (platform_register.rs `new_plan`). */
export type PlatformCreateOutcome = "registered" | "unknown" | "held" | "paidFailure" | "replanRefused";

const short = (needed: bigint, available: bigint) =>
  fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(needed), available: String(available) });

/** input0.rs `plan` for `funding` with input 0 keeping `reserve`: the first funded address above
 * the reserve, then the later ones that hold at least an input's minimum. */
function input0Plan(addresses: [string, bigint][], funding: bigint, reserve: bigint): PlanInput[] {
  const funded = addresses.filter(([, c]) => c > 0n);
  const first = funded.findIndex(([, c]) => c > reserve);
  const usable = first < 0 ? [] : [funded[first], ...funded.slice(first + 1).filter(([, c]) => c >= MIN_INPUT)].slice(0, MAX_INPUTS);
  const held = usable.reduce((n, [, c]) => n + c, 0n);
  if (usable.length === 0 || funding > held - reserve) {
    throw short(funding + reserve, usable.length === 0 ? funded.reduce((n, [, c]) => (c > n ? c : n), 0n) : held);
  }
  const inputs: PlanInput[] = [];
  let left = funding;
  for (const [i, [address, balance]] of usable.entries()) {
    if (left === 0n) break;
    const room = i === 0 ? balance - reserve : balance;
    const claim = room < left ? room : left;
    left -= claim;
    inputs.push({ address, claim: i === 0 || claim >= MIN_INPUT ? claim : MIN_INPUT });
  }
  if (inputs[0].claim < MIN_INPUT) throw short(MIN_INPUT + reserve, usable[0][1]);
  const total = inputs.reduce((n, x) => n + x.claim, 0n);
  if (total < MIN_IDENTITY_FUNDING) throw short(MIN_IDENTITY_FUNDING + reserve, funded.reduce((n, [, c]) => n + c, 0n));
  return inputs;
}

/** registration_platform.rs `register_plan` over the addresses in address order (the fixture's
 * addresses have no bytes: their strings' order stands in for `PlatformAddress`'s): the identity
 * receives at least `funding`; input 0 keeps three fees for exactly the inputs the plan takes. The
 * engine's walk, then any count and input 0 that pays with that count's reserve, fewest first. */
function registerPlan(addresses: PlatformWorld["addresses"], funding: bigint): PlatformRegisterPlan {
  const ordered: [string, bigint][] = [...addresses].sort((a, b) => (a.address < b.address ? -1 : 1)).map((a) => [a.address, BigInt(a.credits)]);
  const wanted = funding > MIN_IDENTITY_FUNDING ? funding : MIN_IDENTITY_FUNDING;
  const planned = (from: [string, bigint][], inputs: number) => {
    const reserve = FEE_BAND * registerFee(inputs);
    const taken = input0Plan(from, wanted, reserve);
    return { taken, reserve };
  };
  const done = ({ taken, reserve }: { taken: PlanInput[]; reserve: bigint }): PlatformRegisterPlan => ({
    funding: taken.reduce((n, x) => n + x.claim, 0n),
    fee: registerFee(taken.length),
    reserve,
    inputs: taken,
  });
  let candidates = ordered;
  let inputs = 1;
  let refusal: unknown = null;
  const tried = new Set<string>();
  for (;;) {
    let plan;
    try {
      plan = planned(candidates, inputs);
    } catch (e) {
      refusal = e;
      break;
    }
    if (plan.taken.length === inputs) return done(plan);
    // A reserve sized for more inputs left the addresses before input 0 out: plan again for fewer
    // without them, so input 0 stays the same address.
    if (plan.taken.length < inputs) candidates = candidates.filter(([a]) => a >= plan.taken[0].address);
    const key = `${inputs}:${candidates.length}`;
    if (tried.has(key)) {
      refusal = short(wanted + plan.reserve, ordered.reduce((n, [, c]) => n + c, 0n));
      break;
    }
    tried.add(key);
    inputs = plan.taken.length;
  }
  for (let n = 1; n <= MAX_INPUTS; n++) {
    for (let i = 0; i < ordered.length; i++) {
      try {
        const plan = planned(ordered.slice(i), n);
        if (plan.taken.length === n) return done(plan);
      } catch {
        // Another count or input 0 may pay.
      }
    }
  }
  throw refusal;
}

/** A Platform-funded row whose create may still execute: its `Register` move holds the addresses
 * until the create settles (C-funding F9, F10). */
const creating = (r: RegistrationStatus) => r.fromPlatform && r.phase === "keysPrepared" && r.fundsCommitted;

/** registration_platform.rs `register_quote`: the plan of a Platform-funded registration of
 * `label`, whose identity receives the name's credits, at least dpp's minimum, from one or more
 * addresses. Refused while a move, or a registration's create, holds the addresses, and off on
 * mainnet (registration_platform.rs `FEATURE`). */
function platformPlan(label: string, p: PlatformWorld | null, mainnet: boolean, dp: DashPayWorld): PlatformRegisterPlan {
  validLabel(label);
  if (mainnet) throw platformOff();
  if (p === null) throw fail("platform.feature_off", "platform", { kind: "feature", feature: "platform" });
  const held = p.activity.find(isHeldOp)?.id ?? dp.registrations.find(creating)?.draft;
  if (held !== undefined) throw fail("credits.move_pending", "a move is pending", { kind: "artifact", artifact: held });
  return registerPlan(p.addresses, costs(label).credits);
}

/** A Platform-funded quote: nothing in duffs; the grant covers the planned debit and the names. */
export function platformQuote(label: string, p: PlatformWorld | null, mainnet: boolean, dp: DashPayWorld): RegistrationQuote {
  const plan = platformPlan(label, p, mainnet, dp);
  const { contested, credits } = costs(label);
  const debit = plan.funding + plan.reserve;
  return {
    contested,
    lockDuffs: 0,
    feeDuffs: 0,
    totalDuffs: 0,
    remainingCredits: String(plan.funding - credits),
    availableDuffs: 0,
    grant: { maxDuffs: 0, maxCredits: String(debit + credits) },
    readiness: null,
    platform: { fundingCredits: String(plan.funding), feeCredits: String(plan.fee), feeReserveCredits: String(plan.reserve), debitCredits: String(debit) },
  };
}

/** What a Platform-funded row's create takes from the addresses (its stored funding and reserve); a
 * scenario's row without a stored plan, one input's. */
function platformDebit(dp: DashPayWorld, row: RegistrationStatus) {
  const stored = dp.platformPlans?.[row.draft];
  const { credits } = costs(row.label);
  const funding = stored?.funding ?? (credits > MIN_IDENTITY_FUNDING ? credits : MIN_IDENTITY_FUNDING);
  const reserve = stored?.reserve ?? FEE_BAND * registerFee(1);
  return { funding, reserve, debit: funding + reserve };
}

/** The fixture quote's costs for `label` (handlers.ts `registration_quote`), and for the temporary
 * name beside it (registration.rs `plan`: only a contested label takes one). The contested lock
 * floor covers both names' credits, so the total does not change. */
export function costs(label: string, temporaryLabel: string | null = null) {
  const contested = checkUsername(label).contested;
  const lockDuffs = contested ? 25_000_000 : 3_000_000;
  const totalDuffs = lockDuffs + 2_260;
  const credits = contested && temporaryLabel !== null ? 2n * NAME_CREDITS : NAME_CREDITS;
  return { contested, lockDuffs, feeDuffs: 2_260, totalDuffs, credits };
}

/** names.rs `check_temporary_name` as registration.rs `plan` applies it: valid, not contested. */
export function checkTemporary(request: RegistrationRequest) {
  const temp = request.temporaryLabel;
  if (temp === null || !checkUsername(request.label).contested) return;
  if (validLabel(temp).contested) throw invalid("the temporary name must not be contested");
}

function changed({ world, emit }: Ctx, walletId: string, draft: string | null) {
  const network = world.launch.network.dirName;
  emit({ kind: "platform", network, walletId, change: { domain: "registration", draft } } satisfies AppEvent);
}

/** The engine's `in_progress` (banner.rs): short of done, and not failed for good. */
const inProgress = (r: RegistrationStatus) => r.phase !== "done" && !(r.failure && !r.failure.retryable);

/** What refuses a start (`registration.in_progress`): a row in progress, or one stopped for good
 * with its funds committed (D22). */
const blocks = (r: RegistrationStatus) => inProgress(r) || (r.phase === "failed" && r.fundsCommitted);

/** The status the banner shows once `rows` changed (m4 §2.1): a finished row's identity, else
 * the row in progress. */
function settleStatus(dp: DashPayWorld) {
  const open = dp.registrations.findLast(inProgress);
  if (open) dp.status = { kind: "registering", draft: open.draft };
  else if (dp.identities.length === 0) dp.status = { kind: "noIdentity", reason: null };
}

export function registrationHandlers(d: RegistrationDeps): Handlers {
  /** Starts a funded run of `request` under `grant` (registration.rs `start`), skipping the
   * wedged blockers in `past`. */
  function start(ctx: Ctx, walletId: string, request: RegistrationRequest, grantId: string, past: string[]): string {
    const { world } = ctx;
    const w = d.wallet(world, walletId);
    const dp = d.dashpay(world, walletId);
    if (request.funding.kind === "invitation") return startClaim(ctx, walletId, request, grantId);
    if (request.funding.kind === "faucetAssetLock") throw fail("platform.feature_off", "faucet", { kind: "feature", feature: "faucet" });
    if (request.funding.kind === "shielded") throw shieldedOff();
    validLabel(request.label);
    checkTemporary(request);
    let plan: PlatformRegisterPlan | null = null;
    if (request.funding.kind === "platformAddresses") {
      plan = platformPlan(request.label, world.pending.platform, world.launch.network.kind === "mainnet", dp);
      // F4: the numbers the quote showed, or nothing is written.
      if (String(plan.funding) !== request.funding.fundingCredits || String(plan.reserve) !== request.funding.feeReserveCredits)
        throw fail("registration.quote_changed", "the quote changed");
    }
    const blocker = dp.registrations.find((r) => blocks(r) && !(past.includes(r.draft) && reported(world, walletId, dp, r).waiting === "fundingUnknown"));
    if (blocker) throw fail("registration.in_progress", "a registration is in progress", { kind: "draft", draft: blocker.draft });
    if (plan) return startFromPlatform(ctx, walletId, request, grantId, plan);
    const { totalDuffs, credits } = costs(request.label, request.temporaryLabel);
    const g = d.grant(world, grantId, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp") throw fail("platform.grant_invalid", "grant invalid");
    const { maxDuffs, maxCredits } = g.grant.purpose;
    if (maxDuffs < totalDuffs)
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "funding", needed: String(totalDuffs), remaining: String(maxDuffs) });
    if (BigInt(maxCredits) < credits)
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(credits), remaining: maxCredits });
    const available = fundable(w);
    if (available < totalDuffs) throw fail("registration.funding_insufficient", "funding insufficient", { kind: "duffs", needed: totalDuffs, available });

    const [, lease] = d.newLease(ctx, walletId, "registration", [g]);
    d.setLease(ctx, { ...lease, fundsCommitted: true, budgets: lease.budgets.map((b) => (b.purpose === "funding" ? { ...b, spent: String(totalDuffs) } : b)) });
    dp.assetLocks += 1;
    const draft = `reg-${dp.assetLocks + dp.registrations.length}`;
    const now = secs(FIXTURE_NOW);
    dp.registrations = [
      ...dp.registrations,
      {
        draft,
        phase: "fundingSent",
        label: request.label,
        temporaryLabel: request.temporaryLabel,
        identity: null,
        txid: dp.assetLocks.toString(16).padStart(2, "0").repeat(32),
        waiting: "instantSend",
        // D26: a run holds a key of its own only under a passphrase grant (`ownKey`); on an
        // unlocked vault it signs with the vault's.
        holdsKey: g.ownKey,
        running: true,
        fundsCommitted: true,
        contestEndsAt: null,
        failure: null,
        nameFailure: null,
        profileFailure: null,
        invitation: null,
        fromPlatform: false,
        createdAt: now,
        updatedAt: now,
      },
    ];
    dp.runs = [...dp.runs, draft];
    settleStatus(dp);
    changed(ctx, walletId, draft);
    return draft;
  }

  /** registration.rs `start` from the Platform balance (C-funding F4, F5): a grant with no duffs
   * that covers the planned debit and the names, and a row whose funding and plan are stored and
   * commit nothing until its create is signed. */
  function startFromPlatform(ctx: Ctx, walletId: string, request: RegistrationRequest, grantId: string, plan: PlatformRegisterPlan): string {
    const { world } = ctx;
    const dp = d.dashpay(world, walletId);
    const credits = plan.funding + plan.reserve + costs(request.label).credits;
    const g = d.grant(world, grantId, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs !== 0) throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < credits)
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(credits), remaining: g.grant.purpose.maxCredits });
    d.newLease(ctx, walletId, "registration", [g]);
    const draft = `reg-${dp.assetLocks + dp.registrations.length + 1}`;
    const now = secs(FIXTURE_NOW);
    dp.platformPlans = { ...dp.platformPlans, [draft]: plan };
    dp.registrations = [
      ...dp.registrations,
      {
        draft,
        phase: "draft",
        label: request.label,
        temporaryLabel: request.temporaryLabel,
        identity: null,
        txid: null,
        waiting: null,
        holdsKey: g.ownKey,
        running: true,
        fundsCommitted: false,
        contestEndsAt: null,
        failure: null,
        nameFailure: null,
        profileFailure: null,
        invitation: null,
        fromPlatform: true,
        createdAt: now,
        updatedAt: now,
      },
    ];
    dp.runs = [...dp.runs, draft];
    settleStatus(dp);
    changed(ctx, walletId, draft);
    return draft;
  }

  /** registration.rs `start` of a claim: its plan, a grant with no duffs (D8 of DP5-02), and a
   * row at `draft` that commits nothing. */
  function startClaim(ctx: Ctx, walletId: string, request: RegistrationRequest, grantId: string): string {
    const { world } = ctx;
    const dp = d.dashpay(world, walletId);
    const blocker = dp.registrations.find(blocks);
    if (blocker) throw fail("registration.in_progress", "a registration is in progress", { kind: "draft", draft: blocker.draft });
    const plan = claimPlan(world, dp, request);
    const g = d.grant(world, grantId, walletId, ["platformOp"]);
    if (g?.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs !== 0) throw fail("platform.grant_invalid", "grant invalid");
    if (BigInt(g.grant.purpose.maxCredits) < plan.credits)
      throw fail("platform.grant_exceeded", "grant exceeded", { kind: "budget", purpose: "credits", needed: String(plan.credits), remaining: g.grant.purpose.maxCredits });
    d.newLease(ctx, walletId, "registration", [g]);
    const draft = `claim-${dp.registrations.length + 1}`;
    const now = secs(FIXTURE_NOW);
    dp.registrations = [
      ...dp.registrations,
      {
        draft,
        phase: "draft",
        label: request.label,
        temporaryLabel: null,
        identity: null,
        txid: null,
        waiting: null,
        holdsKey: g.ownKey,
        running: true,
        fundsCommitted: false,
        contestEndsAt: null,
        failure: null,
        nameFailure: null,
        profileFailure: null,
        invitation: { linkId: plan.linkId, inviter: plan.inviter, requestFailure: null },
        fromPlatform: false,
        createdAt: now,
        updatedAt: now,
      },
    ];
    dp.runs = [...dp.runs, draft];
    settleStatus(dp);
    changed(ctx, walletId, draft);
    return draft;
  }

  return {
    // names.rs: pure and offline.
    check_username: ({ label }) => checkUsername(label),
    name_availability: ({ walletId, label }, { world }) => {
      const dp = d.dashpay(world, walletId);
      if (dp.searchOffline) return { kind: "unknown" };
      return availability(dp, label);
    },
    registrations: ({ walletId }, { world }) => {
      const dp = d.dashpay(world, walletId);
      return dp.registrations.map((r) => reported(world, walletId, dp, r));
    },
    start_registration: ({ walletId, request, grant }, ctx) => start(ctx, walletId, request, grant, []),
    start_registration_past_unknown_funding: ({ walletId, request, grant, past }, ctx) => start(ctx, walletId, request, grant, past),
    // registration.rs `resume`: a finished or running row asks nothing; an idle one runs again
    // under the grant (none: its keyless waits only, which the fixture has none of). No second
    // asset lock: the row's funding is the one it has.
    resume_registration: ({ walletId, draft, grant }, ctx) => {
      const { world } = ctx;
      const dp = d.dashpay(world, walletId);
      const row = dp.registrations.find((r) => r.draft === draft);
      if (!row) throw invalid("unknown draft");
      // F5: a Platform-funded row is off on mainnet before anything else, its grant unredeemed.
      if (row.fromPlatform && world.launch.network.kind === "mainnet") throw platformOff();
      if (row.phase === "failed" && !row.failure?.retryable) throw invalid(`registration ${draft} failed for good (${row.failure?.code})`);
      if (row.phase === "done" || active(world, walletId, dp, draft) || grant === null) return null;
      // F5: a Platform-funded row's grant carries no duffs.
      const g = d.grant(world, grant, walletId, ["platformOp"]);
      if (!g) throw fail("platform.grant_invalid", "grant invalid");
      if (row.fromPlatform && (g.grant.purpose.kind !== "platformOp" || g.grant.purpose.maxDuffs !== 0)) throw fail("platform.grant_invalid", "grant invalid");
      // A parked run takes the grant (a new lease); an idle row runs again.
      d.newLease(ctx, walletId, "registration", [g]);
      const phase = row.phase === "failed" ? row.failure!.phase : row.phase;
      Object.assign(row, { phase, failure: null, waiting: runningWait(phase), holdsKey: g.ownKey, running: true, updatedAt: secs(FIXTURE_NOW) });
      dp.runs = [...dp.runs.filter((x) => x !== draft), draft];
      settleStatus(dp);
      changed(ctx, walletId, draft);
      return null;
    },
    // registration.rs `resume_grant` (DP5-03 D20): a claim row's or a Platform-funded row's
    // grant from the row alone, no duffs and the credits of the steps it has left.
    registration_resume_grant: ({ walletId, draft }, { world }) => {
      const row = d.dashpay(world, walletId).registrations.find((r) => r.draft === draft);
      if (!row) throw invalid("unknown draft");
      if (!row.invitation && !row.fromPlatform) throw invalid("not an invitation's claim or a Platform-funded row");
      if (row.fromPlatform && world.launch.network.kind === "mainnet") throw platformOff();
      const at = PHASE_ORDER.indexOf(row.phase === "failed" ? row.failure!.phase : row.phase);
      let credits = row.identity ? 0n : row.fromPlatform ? platformDebit(d.dashpay(world, walletId), row).debit : IDENTITY_CREDITS;
      if (at < PHASE_ORDER.indexOf("nameRegistered")) credits += NAME_CREDITS;
      if (row.invitation?.inviter && at < PHASE_ORDER.indexOf("done")) credits += REQUEST_CREDITS;
      return { maxDuffs: 0, maxCredits: String(credits) };
    },
    // registration.rs `discard`: only a row whose funds are not committed.
    discard_registration: ({ walletId, draft }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId);
      const row = dp.registrations.find((r) => r.draft === draft);
      if (!row) throw invalid("unknown draft");
      if (row.fundsCommitted) throw invalid("the registration's funds are committed");
      // DP5-03 D11: a claim whose identity exists finishes its name first.
      if (row.invitation && row.identity && row.phase !== "done") throw invalid("the claim's identity exists; its name is not registered yet");
      dp.registrations = dp.registrations.filter((r) => r !== row);
      dp.runs = dp.runs.filter((x) => x !== draft);
      settleStatus(dp);
      changed(ctx, walletId, draft);
      return null;
    },
    // Tools ▸ Repair "Finish transfers": every row whose funds wait for a decision runs again, and so does every ready lock and every restored checking one.
    finish_asset_locks: ({ walletId, grant }, ctx) => {
      const { world } = ctx;
      const dp = d.dashpay(world, walletId);
      const g = d.grant(world, grant, walletId, ["platformOp"]);
      if (!g) throw fail("platform.grant_invalid", "grant invalid");
      const stranded = dp.registrations.filter((r) => !dp.runs.includes(r.draft) && r.waiting === "needsFinish");
      d.newLease(ctx, walletId, "registration", [g]);
      for (const row of stranded) {
        Object.assign(row, { waiting: runningWait(row.phase), holdsKey: g.ownKey, running: true });
        dp.runs = [...dp.runs, row.draft];
        changed(ctx, walletId, row.draft);
      }
      return reportOf({ resumed: stranded.length, completed: 0, stillPending: 0 }, finishRestored(ctx, walletId), world.pending.recovery.unread);
    },
    // names.rs `register_name`: a name for an identity that has one or none, paid from its credits.
    register_name: ({ walletId, identity, label, grant }, ctx) => {
      const dp = d.dashpay(ctx.world, walletId, identity);
      const check = validLabel(label);
      const known = availability(dp, label);
      if (known.kind === "taken") throw fail("name.taken", "another identity holds the label");
      if (known.kind === "contestOpen") throw fail("name.contest_open", "a contest for the name is open");
      if (known.kind === "locked") throw fail("name.locked", "name locked");
      const me = dp.identities.find((i) => i.identity === identity)!;
      if (BigInt(me.balance ?? "0") < NAME_CREDITS)
        throw fail("platform.insufficient_credits", "insufficient credits", { kind: "credits", needed: String(NAME_CREDITS), available: me.balance ?? "0" });
      d.writeLease(ctx, walletId, grant, ["nameRegistration"], NAME_CREDITS)();
      const network = ctx.world.launch.network.dirName;
      if (check.contested) {
        const endsAt = secs(FIXTURE_NOW) + CONTEST_SECS;
        dp.status = { kind: "contestPending", identity, label, endsAt };
        ctx.emit({ kind: "platform", network, walletId, change: { domain: "names" } });
        return { kind: "contestStarted", endsAt };
      }
      dp.identities = dp.identities.map((i) => (i.identity === identity ? { ...i, names: [...i.names, label], mainName: i.mainName ?? label, namesCheck: null } : i));
      dp.status = { kind: "ready", main: identity };
      ctx.emit({ kind: "platform", network, walletId, change: { domain: "names" } });
      ctx.emit({ kind: "platform", network, walletId, change: { domain: "identities" } });
      return { kind: "registered" };
    },
  };
}

/** One step of `draft`'s run, where the network would answer (a test's `advance`): the
 * InstantSend lock arrives, its proof creates the identity, then the name goes out and is
 * registered (or its contest starts) and the row is done. `false` when it has no active run. */
export function advance(ctx: Ctx, walletId: string, draft: string, d: Pick<RegistrationDeps, "dashpay" | "setLease">): boolean {
  const { world } = ctx;
  const dp = d.dashpay(world, walletId);
  const row = dp.registrations.find((r) => r.draft === draft);
  if (!row || !active(world, walletId, dp, draft)) return false;
  const now = secs(FIXTURE_NOW);
  const network = world.launch.network.dirName;
  // A claim checks its invitation (draft), then claims it (keysPrepared): no lock, no proof.
  const claim = row.invitation !== null;
  // A Platform-funded row's funding step is its identity create (C-funding F9, F10).
  const step = row.phase === "keysPrepared" ? (claim ? "proofWaiting" : row.fromPlatform ? "platformCreate" : row.phase) : row.phase;
  switch (step) {
    case "draft":
      if (claim || row.fromPlatform) row.phase = "keysPrepared";
      else Object.assign(row, { phase: "proofWaiting", waiting: runningWait("proofWaiting") });
      break;
    case "platformCreate": {
      const outcome = dp.platformCreates?.shift() ?? "registered";
      const plan = dp.platformPlans?.[row.draft];
      const p = world.pending.platform;
      if (outcome === "paidFailure") {
        // registration_flow.rs `PaidFailure` with no identity at the row's index: the fee leaves
        // input 0 (dpp's penalty is not modelled) and the row is cleared with `registration.refunded`.
        if (plan && p) {
          const source = plan.inputs[0].address;
          p.addresses = p.addresses.map((a) => (a.address === source ? { ...a, credits: String(BigInt(a.credits) - plan.fee) } : a));
          ctx.emit({ kind: "platform", network, walletId, change: { domain: "platformAddresses" } });
        }
        const failure: RegistrationFailure = { phase: row.phase, code: "registration.refunded", retryable: false, needed: null, available: null };
        Object.assign(row, { phase: "failed", fundsCommitted: false, failure });
        end(ctx, walletId, dp, row, d);
        settleStatus(dp);
        break;
      }
      if (outcome === "replanRefused") {
        const failure: RegistrationFailure = { phase: row.phase, code: "registration.quote_changed", retryable: false, needed: null, available: null };
        Object.assign(row, { phase: "failed", fundsCommitted: false, failure });
        end(ctx, walletId, dp, row, d);
        settleStatus(dp);
        break;
      }
      if (outcome !== "registered") {
        // F7: an unknown answer waits and reads again; F22: one failed by height parks for the user.
        Object.assign(row, { waiting: outcome === "unknown" ? "paymentUnknown" : "paymentHeld", fundsCommitted: true });
        if (outcome === "held") park(ctx, walletId, dp, row, d);
        break;
      }
      if (plan && p) {
        // Drive takes each claim, and the fee from input 0.
        const taken = new Map(plan.inputs.map((x, i) => [x.address, x.claim + (i === 0 ? plan.fee : 0n)]));
        p.addresses = p.addresses.map((a) => (taken.has(a.address) ? { ...a, credits: String(BigInt(a.credits) - taken.get(a.address)!) } : a));
        ctx.emit({ kind: "platform", network, walletId, change: { domain: "platformAddresses" } });
      }
      Object.assign(row, { phase: "identityRegistered", identity: NEW_IDENTITY, waiting: null, fundsCommitted: true });
      dp.identities = [createdIdentity(String(platformDebit(dp, row).funding))];
      ctx.emit({ kind: "platform", network, walletId, change: { domain: "identities" } });
      break;
    }
    case "keysPrepared":
    case "fundingSent":
      Object.assign(row, { phase: "proofWaiting", waiting: runningWait("proofWaiting") });
      break;
    case "proofWaiting":
      Object.assign(row, { phase: "identityRegistered", identity: NEW_IDENTITY, waiting: null });
      // DP5-02 D13: the link is wiped once the claim's identity is recorded.
      if (row.invitation) world.stash = world.stash.filter((s) => s.info.id !== row.invitation!.linkId);
      dp.identities = [createdIdentity(String(costs(row.label).lockDuffs * 1000))];
      ctx.emit({ kind: "platform", network, walletId, change: { domain: "identities" } });
      break;
    case "identityRegistered":
      row.phase = "nameRequested";
      break;
    case "nameRequested":
      if (costs(row.label).contested) {
        Object.assign(row, { phase: "contested", contestEndsAt: now + CONTEST_SECS });
        // The temporary name right after ContestStarted; a refusal is only logged.
        const temp = row.temporaryLabel;
        if (temp !== null && !dp.temporaryRefused) {
          dp.identities = dp.identities.map((i) => (i.identity === row.identity ? { ...i, names: [...i.names, temp], mainName: temp } : i));
          ctx.emit({ kind: "platform", network, walletId, change: { domain: "names" } });
        }
      } else {
        row.phase = "nameRegistered";
        dp.identities = dp.identities.map((i) => (i.identity === row.identity ? { ...i, names: [row.label], mainName: row.label } : i));
        ctx.emit({ kind: "platform", network, walletId, change: { domain: "names" } });
      }
      break;
    case "nameRegistered":
    case "contested":
    case "profileCreated":
      // DP5-02 D15: the request to the inviter, last; refused for good, the row is done anyway.
      if (row.invitation?.inviter) row.invitation = { ...row.invitation, requestFailure: dp.requestRefusal ?? null };
      end(ctx, walletId, dp, row, d);
      dp.status = row.contestEndsAt !== null
        ? { kind: "contestPending", identity: row.identity!, label: row.label, endsAt: row.contestEndsAt }
        : { kind: "ready", main: row.identity! };
      break;
    default:
      return false;
  }
  row.updatedAt = now;
  changed(ctx, walletId, draft);
  return true;
}

/** `draft`'s run fails in its phase (a test's `failRegistration`): `retryable` keeps the row for
 * Continue; a name refusal (`name.*`) ends it `done` with `nameFailure`, the identity kept. */
export function failRun(ctx: Ctx, walletId: string, draft: string, code: string, retryable: boolean, d: Pick<RegistrationDeps, "dashpay" | "setLease">): boolean {
  const dp = d.dashpay(ctx.world, walletId);
  const row = dp.registrations.find((r) => r.draft === draft);
  if (!row || finished(row)) return false;
  if (code.startsWith("name.")) {
    Object.assign(row, { nameFailure: code });
    end(ctx, walletId, dp, row, d);
    if (row.identity) dp.status = { kind: "ready", main: row.identity };
  } else {
    const failure: RegistrationFailure = { phase: row.phase, code, retryable, needed: null, available: null };
    Object.assign(row, { phase: "failed", failure });
    end(ctx, walletId, dp, row, d);
    settleStatus(dp);
  }
  row.updatedAt = secs(FIXTURE_NOW);
  changed(ctx, walletId, draft);
  return true;
}

/** The identity a registration created, holding `balance` credits. */
const createdIdentity = (balance: string): IdentitySummary => ({
  identity: NEW_IDENTITY,
  index: 0,
  names: [],
  mainName: null,
  namesUpdating: false,
  namesCheck: null,
  isMain: true,
  balance,
  balanceAsOf: FIXTURE_NOW.getTime(),
  balanceStale: false,
  hasDashpayKeys: true,
  profile: null,
  unverified: false,
  creditLevel: "ok",
});

/** The run parks (registration_flow.rs `Step::Park`): it ends with its lease, and the row keeps
 * its phase and wait for the user. */
function park(ctx: Ctx, walletId: string, dp: DashPayWorld, row: RegistrationStatus, d: Pick<RegistrationDeps, "setLease">) {
  Object.assign(row, { holdsKey: false, running: false });
  dp.runs = dp.runs.filter((x) => x !== row.draft);
  const lease = runLease(ctx.world, walletId);
  if (lease) d.setLease(ctx, { ...lease, state: { kind: "ended" }, keyExpiresInSecs: null });
}

/** The run ends: its row settles, its lease ends. */
function end(ctx: Ctx, walletId: string, dp: DashPayWorld, row: RegistrationStatus, d: Pick<RegistrationDeps, "setLease">) {
  if (row.phase !== "failed") row.phase = "done";
  row.waiting = null;
  park(ctx, walletId, dp, row, d);
}
