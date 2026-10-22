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
// Not modelled: profiles (F2), temporary names (D4), the funding's coin selection and its debit
// from the balance, InstantSend timeouts, and proofs (a resumed `fundingSent` row waits for its
// InstantSend lock again).

import type { AppError, AppEvent } from "../api/generated/commands";
import type { FlowKindName } from "../api/generated/FlowKindName";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { InvitationStatus } from "../api/generated/InvitationStatus";
import type { Lease } from "../api/generated/Lease";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RegistrationWaitName } from "../api/generated/RegistrationWaitName";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import type { UsernameRuleName } from "../api/generated/UsernameRuleName";
import { IDENTITY, type DashPayWorld } from "./dashpay";
import type { Ctx, Handlers } from "./handlers";
import type { FixtureGrant, FixtureWallet, World } from "./scenarios";
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
  const kept = r.waiting === "needsFinish" || r.waiting === "fundingUnknown";
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

/** The fixture quote's costs for `label` (handlers.ts `registration_quote`). */
export function costs(label: string) {
  const contested = checkUsername(label).contested;
  const lockDuffs = contested ? 25_000_000 : 3_000_000;
  const totalDuffs = lockDuffs + 2_260;
  return { contested, lockDuffs, feeDuffs: 2_260, totalDuffs, credits: NAME_CREDITS };
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
    validLabel(request.label);
    const blocker = dp.registrations.find((r) => blocks(r) && !(past.includes(r.draft) && reported(world, walletId, dp, r).waiting === "fundingUnknown"));
    if (blocker) throw fail("registration.in_progress", "a registration is in progress", { kind: "draft", draft: blocker.draft });
    const { totalDuffs, credits } = costs(request.label);
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
      if (row.phase === "failed" && !row.failure?.retryable) throw invalid(`registration ${draft} failed for good (${row.failure?.code})`);
      if (row.phase === "done" || active(world, walletId, dp, draft) || grant === null) return null;
      const g = d.grant(world, grant, walletId, ["platformOp"]);
      if (!g) throw fail("platform.grant_invalid", "grant invalid");
      // A parked run takes the grant (a new lease); an idle row runs again.
      d.newLease(ctx, walletId, "registration", [g]);
      const phase = row.phase === "failed" ? row.failure!.phase : row.phase;
      Object.assign(row, { phase, failure: null, waiting: runningWait(phase), holdsKey: g.ownKey, running: true, updatedAt: secs(FIXTURE_NOW) });
      dp.runs = [...dp.runs.filter((x) => x !== draft), draft];
      settleStatus(dp);
      changed(ctx, walletId, draft);
      return null;
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
    // Tools ▸ Repair "Finish transfers": every row whose funds wait for a decision runs again.
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
      return { resumed: stranded.length, completed: 0, stillPending: 0 };
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
  const step = claim && row.phase === "keysPrepared" ? "proofWaiting" : row.phase;
  switch (claim && row.phase === "draft" ? "claimDraft" : step) {
    case "claimDraft":
      row.phase = "keysPrepared";
      break;
    case "draft":
    case "keysPrepared":
    case "fundingSent":
      Object.assign(row, { phase: "proofWaiting", waiting: runningWait("proofWaiting") });
      break;
    case "proofWaiting":
      Object.assign(row, { phase: "identityRegistered", identity: NEW_IDENTITY, waiting: null });
      // DP5-02 D13: the link is wiped once the claim's identity is recorded.
      if (row.invitation) world.stash = world.stash.filter((s) => s.info.id !== row.invitation!.linkId);
      dp.identities = [
        {
          identity: NEW_IDENTITY,
          index: 0,
          names: [],
          mainName: null,
          namesUpdating: false,
          namesCheck: null,
          isMain: true,
          balance: String(costs(row.label).lockDuffs * 1000),
          balanceAsOf: FIXTURE_NOW.getTime(),
          balanceStale: false,
          hasDashpayKeys: true,
          profile: null,
          unverified: false,
          creditLevel: "ok",
        },
      ];
      ctx.emit({ kind: "platform", network, walletId, change: { domain: "identities" } });
      break;
    case "identityRegistered":
      row.phase = "nameRequested";
      break;
    case "nameRequested":
      if (costs(row.label).contested) Object.assign(row, { phase: "contested", contestEndsAt: now + CONTEST_SECS });
      else {
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

/** The run ends: its row settles, its lease ends. */
function end(ctx: Ctx, walletId: string, dp: DashPayWorld, row: RegistrationStatus, d: Pick<RegistrationDeps, "setLease">) {
  if (row.phase !== "failed") row.phase = "done";
  Object.assign(row, { waiting: null, holdsKey: false, running: false });
  dp.runs = dp.runs.filter((x) => x !== row.draft);
  const lease = runLease(ctx.world, walletId);
  if (lease) d.setLease(ctx, { ...lease, state: { kind: "ended" }, keyExpiresInSecs: null });
}
