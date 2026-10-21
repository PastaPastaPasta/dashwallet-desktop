// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// Named fixture scenarios. Each builds a fresh `World`: the state the fixture's command
// handlers read and change. Pick one with `?scenario=<name>` (dev server, Playwright) or
// `installFixture({ scenario })` (vitest).

import type { AuthGrant } from "../api/generated/AuthGrant";
import type { ContestStatus } from "../api/generated/ContestStatus";
import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { PendingTopUp } from "../api/generated/PendingTopUp";
import type { QuorumSource } from "../api/generated/QuorumSource";
import type { DispatchStateName } from "../api/generated/DispatchStateName";
import type { FlowLockReport } from "../api/generated/FlowLockReport";
import type { FlowOutcomeName } from "../api/generated/FlowOutcomeName";
import type { LaunchInfo } from "../api/generated/LaunchInfo";
import type { LinkRoute } from "../api/generated/LinkRoute";
import { stashRoute, type FixtureStashedLink } from "./links";
import type { Lease } from "../api/generated/Lease";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { LockStatus } from "../api/generated/LockStatus";
import type { RecentNotice } from "../api/generated/RecentNotice";
import type { ShellSync } from "../api/generated/ShellSync";
import type { SyncPassReport } from "../api/generated/SyncPassReport";
import type { SyncStatus } from "../api/generated/SyncStatus";
import type { ThemeName } from "../api/generated/ThemeName";
import type { TxRow } from "../api/generated/TxRow";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { IDENTITY, noIdentity, registered, registering, type DashPayWorld } from "./dashpay";
import { newCreditsWorld, topUpStep, type CreditsWorld } from "./topup";
import type { Utxo } from "../api/generated/Utxo";
import { buildCoins, newReceiveWorld, newSendWorld, type ReceiveWorld, type SendWorld } from "./send";
import { newToolsWorld, type ToolsWorld } from "./tools";
import { newSettingsWorld, withAutomaticBackups, type SettingsWorld } from "./settings";
import { scanWorld, type ScanWorld } from "./scan";
import { coinjoinScenarios, newCoinJoinWorld, type CoinJoinWorld } from "./coinjoin";
import { governanceScenarios, newGovernanceWorld, type GovernanceWorld } from "./governance";
import { MAIN_PHRASE, SECOND_PHRASE, throttleWaitSecs } from "./vault";
import { buildRows, caughtUp, FIXTURE_NOW, FUNDED_BALANCES, makeIds, prng, secs, XSS_PAYLOADS, ZERO_BALANCES, type HistoryVariant } from "./wallet";

export const FIXTURE_PASSPHRASE = "demo";
export const FIXTURE_WALLET_ID = "a1".repeat(32);
/** The `two-wallets` scenario's second wallet. */
export const SECOND_WALLET_ID = "b2".repeat(32);

export interface FixtureWallet {
  summary: WalletSummary;
  rows: TxRow[];
  coins: Utxo[];
  receive: ReceiveWorld;
}

/** A 4 x 4 PNG in the DashPay blue: the fixture's avatar thumbnail (DP4-02). */
export const AVATAR_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGOQKd3zHxkzkC4AAMqAJMErkdEyAAAAAElFTkSuQmCC";

/** What the fixture's engine knows. Handlers mutate it, like the engine's own state. */
export interface World {
  launch: LaunchInfo;
  passphrase: string;
  lock: LockStatus;
  /** The recovery phrase of each wallet whose secrets the vault holds (U-02). */
  phrases: Record<string, string>;
  wallets: FixtureWallet[];
  sync: SyncStatus;
  /** The first wallet's DashPay state. */
  dashpay: DashPayWorld;
  /** Another wallet's own DashPay state, by wallet id; a wallet not here shares `dashpay`. */
  dashpayOf?: Record<string, DashPayWorld>;
  /** The session's quorum source and the developer toggle (E0-10b; DP6-03). The default is the
   * degraded mode's `trusted` (DEC-168). */
  trust: { source: QuorumSource; trustedOnly: boolean };
  leases: Lease[];
  /** The vault grants `authorize` issued since the last lock or unlock (each revokes them all). */
  grants: FixtureGrant[];
  /** `begin_flow`'s lease ids (`lease-` and 32 hex digits) to their `Lease.id` in `leases`. */
  flowLeases: Record<string, string>;
  /** Grant and lease ids issued so far: they are drawn from it, so runs repeat. */
  issued: number;
  /** `dispatch_status` by artifact. */
  dispatch: Record<string, DispatchStateName>;
  lastLockReport: LockOutcome | null;
  /** What the next `lock` cuts and reports (E0-13 D3's bounded report). */
  cut: Pick<LockOutcome, "flows" | "flowsOmitted">;
  notices: RecentNotice[];
  /** Deep links waiting for `take_links` (`FixtureHandle.deliverLinks`), already routed. */
  links: LinkRoute[];
  /** The engine's link stash (DP5-01): links kept for after onboarding, and invitations. */
  stash: FixtureStashedLink[];
  /** The last `shell_sync`: what the native menus would show. */
  shellSync: ShellSync | null;
  /** "Require authentication for every payment" (E0-13 D11; on by default). */
  paymentAuth: boolean;
  /** "Load contact pictures" (DP4-02 D17); absent: on. */
  loadContactPictures?: boolean;
  /** The vault's wait after a wrong passphrase (`retryAfterSecs`); null: none. */
  throttleSecs: number | null;
  /** When that wait ends (ms since epoch), for `lock_status`; null: no wait. */
  throttledUntil: number | null;
  /** Drafts, prepared transactions and what the next broadcast does (send.ts). */
  send: SendWorld;
  /** Tools: node facts, console, peers, repair, address book, messages, PSBT, files (tools.ts). */
  tools: ToolsWorld;
  /** The settings file, load states, backups and files (settings.ts, U-04). */
  settings: SettingsWorld;
  /** The QR scan commands' screen picker (scan.ts, DP2-08). */
  scan: ScanWorld;
  /** CoinJoin's options and each wallet's mixer (coinjoin.ts, U-06). */
  coinjoin: CoinJoinWorld;
  /** Governance: sync, proposals, voting masternodes and own proposals (governance.ts, MG-09). */
  governance: GovernanceWorld;
}

/** A vault grant: single use (`used` once redeemed or revoked); `ownKey` when it was issued
 * while the vault held no data key, so it carries its own. */
export interface FixtureGrant {
  grant: AuthGrant;
  used: boolean;
  ownKey: boolean;
}

/** dw-app's `MAX_REPORT_WALLETS`: a lock report lists this many wallets and counts the rest. */
export const MAX_REPORT_WALLETS = 16;

const unlocked = (walletIds: string[]): LockStatus => ({
  state: "unlocked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: walletIds,
});

function wallet(variant: HistoryVariant): FixtureWallet {
  return {
    summary: {
      walletId: FIXTURE_WALLET_ID,
      name: variant === "xss" ? XSS_PAYLOADS.walletName : "Main",
      watchOnly: false,
      hasMnemonic: true,
      hd: true,
      birthHeight: 1_200_000,
      createdAt: null,
      balances: variant === "empty" ? ZERO_BALANCES : FUNDED_BALANCES,
    },
    rows: buildRows(variant),
    coins: buildCoins(variant),
    receive: newReceiveWorld(),
  };
}

/** A wallet `import_wallet` adds: no history yet, and its birth height the tip's. */
export function emptyWallet(walletId: string, name: string): FixtureWallet {
  const w = wallet("empty");
  w.summary = { ...w.summary, walletId, name };
  return w;
}

/** The funded wallet behind the lock screen after `failedAttempts` wrong passphrases, with the
 * wait dw-vault reports for them (the fixture's clock stands still, so it never runs out). */
function lockedAfter(w: World, failedAttempts: number): World {
  w.lock = { ...w.lock, state: "locked", failedAttempts, retryAfterSecs: throttleWaitSecs(failedAttempts) || null };
  return w;
}

function base(theme: ThemeName | null, variant: HistoryVariant = "default"): World {
  const main = wallet(variant);
  return {
    launch: {
      selftest: false,
      selftestPauseMs: 0,
      selftestUnlock: false,
      selftestLinkTrace: false,
      theme,
      // Testnet, like the reference images.
      network: { kind: "testnet", dirName: "testnet", unitName: "tDASH" },
      appVersion: "0.1.0",
      engineVersion: "fixture",
    },
    passphrase: FIXTURE_PASSPHRASE,
    lock: unlocked([FIXTURE_WALLET_ID]),
    phrases: { [FIXTURE_WALLET_ID]: MAIN_PHRASE },
    wallets: [main],
    sync: caughtUp(),
    dashpay: noIdentity(),
    trust: { source: "trusted", trustedOnly: false },
    leases: [],
    grants: [],
    flowLeases: {},
    issued: 0,
    dispatch: {},
    lastLockReport: null,
    cut: { flows: [], flowsOmitted: 0 },
    notices: [],
    links: [],
    stash: [],
    shellSync: null,
    paymentAuth: true,
    throttleSecs: null,
    throttledUntil: null,
    send: newSendWorld(),
    tools: newToolsWorld(main),
    settings: withAutomaticBackups(newSettingsWorld(), FIXTURE_WALLET_ID, MAIN_PHRASE, FIXTURE_PASSPHRASE),
    scan: scanWorld(),
    coinjoin: newCoinJoinWorld(),
    governance: newGovernanceWorld(),
  };
}

const lease = (id: string, flow: Lease["flow"], fundsCommitted: boolean, inFlight = 0): Lease => ({
  id,
  walletId: FIXTURE_WALLET_ID,
  flow,
  state: { kind: "active" },
  ownKey: true,
  keyExpiresInSecs: 240,
  fundsCommitted,
  budgets: [{ purpose: "funding", ceiling: "150000000", spent: fundsCommitted ? "120000000" : "0" }],
  inFlight,
  callRunning: inFlight > 0,
});

/** The engine's outcome order, weakest first (E0-04 §8.4): a flow's is the strongest of its
 * artifacts'. And dw-app's listing order, unresolved first (`keep_unresolved_first`). */
const STRENGTH: FlowOutcomeName[] = ["cancelled", "maybeSent", "willBeSent", "sent"];
const LISTING: FlowOutcomeName[] = ["maybeSent", "willBeSent", "sent", "cancelled"];
const listed = <T extends { outcome: FlowOutcomeName }>(items: T[]) => items.toSorted((a, b) => LISTING.indexOf(a.outcome) - LISTING.indexOf(b.outcome));

/** A 32-byte artifact id (a txid or a state-transition hash): plain lower-case hex. */
const artifactId = (seed: string) => seed.repeat(64 / seed.length);
/** A step id, as a running call or a funding step is named. */
const step = (flow: string, draft: string, part: string) => `${flow}/${draft}/${part}`;

/** A cut flow as the bridge reports it: the facade's lease id (`lease-` and 32 hex digits, the
 * first 8 being `Lease.id`), the outcome derived from the artifacts, those listed unresolved first. */
const flow = (lease: string, kind: FlowLockReport["flow"], artifacts: [string, FlowOutcomeName][], omitted = 0): FlowLockReport => ({
  lease: `lease-${lease.repeat(4)}`,
  walletId: FIXTURE_WALLET_ID,
  flow: kind,
  outcome: artifacts.reduce<FlowOutcomeName>((o, [, a]) => (STRENGTH.indexOf(a) > STRENGTH.indexOf(o) ? a : o), "cancelled"),
  artifacts: listed(artifacts.map(([artifact, outcome]) => ({ artifact, outcome }))),
  artifactsOmitted: omitted,
});

/** Locks `w` with these flows cut, listed as the bridge lists them; the lock revokes each flow's
 * lease. The dispatch state of every listed artifact starts at its outcome. With `walletsOmitted`
 * above 0 the vault holds that many wallets with secrets beyond the 16 a report lists. */
function cutting(w: World, flows: FlowLockReport[], flowsOmitted = 0, walletsOmitted = 0): World {
  if (walletsOmitted > 0) {
    const others = Array.from({ length: MAX_REPORT_WALLETS - 1 + walletsOmitted }, (_, i) => hex(i + 1).repeat(32));
    w.lock = { ...w.lock, walletsWithSecrets: [...w.lock.walletsWithSecrets, ...others].sort() };
  }
  w.dashpay = registered();
  w.leases = flows.map((f) => lease(f.lease.slice("lease-".length, "lease-".length + 8), f.flow, f.flow === "registration" || f.flow === "topUp" || f.flow === "acceptAndPay"));
  w.dispatch = Object.fromEntries(flows.flatMap((f) => f.artifacts.map((a) => [a.artifact, a.outcome === "cancelled" ? "notSent" : a.outcome])));
  w.cut = { flows: listed(flows), flowsOmitted };
  return w;
}

const hex = (i: number) => i.toString(16).padStart(2, "0");
/** A contact request cut by the lock: its state transition, with this outcome. */
const request = (i: number, outcome: FlowOutcomeName) =>
  flow(`5eed00${hex(i)}`, "contactRequest", outcome === "cancelled" ? [] : [[artifactId(`5eed00${hex(i)}`), outcome]]);

/** A registration whose funding was sent and whose identity call may have gone out: the engine
 * lists the funding step too, so it reads `sent` (E0-04 §8.4). */
const registrationCut = () =>
  flow("3f9a1c27", "registration", [
    [step("registration", "3f9a1c27", "funding"), "sent"],
    [step("registration", "3f9a1c27", "identity"), "maybeSent"],
  ]);

/** The cut flows of a full report (E0-13 D3: 16 flows of 8 artifacts): a registration whose
 * identity call was still running, a top-up with these artifact outcomes, and 14 requests. */
function sixteen(requests: FlowOutcomeName, topUp: FlowOutcomeName[], topUpOmitted: number): FlowLockReport[] {
  return [
    registrationCut(),
    flow("8be04d11", "topUp", topUp.map((o, i) => [artifactId(`8be04d${hex(i)}`), o]), topUpOmitted),
    ...Array.from({ length: 14 }, (_, i) => request(i, requests)),
  ];
}

/** Every listed flow and artifact is pending and more were cut: the omitted ones may be pending
 * too, and no query names them. Unresolved for good. */
const omittedPending = (w: World) => cutting(w, sixteen("willBeSent", Array(8).fill("willBeSent"), 7), 3, 2);

/** Counts above 0, but the order proves nothing omitted is pending: the listed flows end with
 * cancelled ones and the top-up's listed artifacts with sent ones (the top-up itself reads
 * `sent`, its strongest). Unresolved until the listed pending artifacts settle and the
 * registration's call, still running, ends. */
function omittedSettled(w: World): World {
  cutting(w, sixteen("cancelled", ["willBeSent", ...Array<FlowOutcomeName>(7).fill("sent")], 4), 2, 2);
  w.leases[0] = { ...w.leases[0], inFlight: 1, callRunning: true };
  return w;
}

/** Flows in flight when the user locks, all listed: a registration whose funding was sent while
 * its identity call may have gone out (E0-04 §8.4: it reads `sent`, yet something of it is
 * pending), a top-up handed off, a sent payment and a cancelled request. */
const listedFlows = (w: World) =>
  cutting(w, [
    registrationCut(),
    flow("8be04d11", "topUp", [[artifactId("8be04d11"), "willBeSent"]]),
    flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]),
    request(0, "cancelled"),
  ]);

/** An accept-and-pay whose payment was sent while its `accept_request` call may have gone out.
 * A call is reported under its label, which `dispatchStatus` cannot name: unresolved for good. */
const unnamedCall = (w: World) =>
  cutting(w, [
    flow("c0ffee01", "acceptAndPay", [
      [artifactId("c0ffee01"), "sent"],
      ["accept_request", "maybeSent"],
    ]),
  ]);

/** Everything resolved: the payment was sent and the request cancelled. */
function resolvedFlows(w: World): World {
  return cutting(w, [flow("c0ffee01", "acceptAndPay", [[artifactId("c0ffee01"), "sent"]]), request(0, "cancelled")]);
}

/** The engine's counterparty attribution (DP3-02) for the first unlabelled payment from Bob
 * (named "Bob Builder") and to Dave (no display name: `@dave`). */
function attributePayments(w: World) {
  const party = (name: "bob" | "dave") => {
    const c = w.dashpay.contacts.find((x) => x.summary.contact === IDENTITY[name])!.summary;
    return { identity: c.contact, username: c.username, displayName: c.displayName, avatarUrl: null };
  };
  const rows = w.wallets[0].rows;
  const first = (type: TxRow["txType"]) => rows.find((r) => r.txType === type && !r.label && r.status.kind === "confirmed");
  const attribute = (row: TxRow, name: "bob" | "dave", direction: "sent" | "received") => {
    row.counterparty = party(name);
    // The contact's activity lists it too (DP2-09), so its row opens the transaction.
    const items = (w.dashpay.activity[IDENTITY[name]] ??= []);
    items.push({ txid: row.txid, direction, amount: Math.abs(row.amount) - (direction === "sent" ? (row.fee ?? 0) : 0), height: row.blockHeight, timestamp: row.timestamp });
  };
  attribute(first("recvWithAddress")!, "bob", "received");
  attribute(first("sendToAddress")!, "dave", "sent");
}

/** `dashpay-registered` with Alice's avatar published and verified (DP4-02). */
function withAvatar(t: ThemeName | null): World {
  const w = base(t);
  const world = registered();
  const alice = world.identities.find((i) => i.identity === IDENTITY.alice)!;
  alice.profile = {
    ...alice.profile!,
    avatarUrl: "https://example.org/alice.png",
    avatarHash: "ab".repeat(32),
    avatarFingerprint: "cd".repeat(8),
  };
  w.dashpay = { ...world, avatars: { [IDENTITY.alice]: AVATAR_PNG } };
  return w;
}

/** Solid 8 x 8 PNGs (green, red, purple, orange): the other users' thumbnails (DP4-03). */
const PICTURES = [
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mPQqzXCihiGlgQAlgM3QYr963wAAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mM4pqGBFTEMLQkAT91FgcmWcpUAAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mPIkp6FFTEMLQkA/V1HwaOKzf8AAAAASUVORK5CYII=",
  "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mN4n8OAFTEMLQkAIYZWwaEHDpcAAAAASUVORK5CYII=",
];

/** `dashpay-avatar` with the history's counterparties (DP3-07) and everyone's picture but
 * Carol's; Erin's bytes do not decode. `on`: "Load contact pictures". */
function withPictures(t: ThemeName | null, on: boolean): World {
  const w = withAvatar(t);
  attributeMore(w);
  const others = Object.entries(IDENTITY).filter(([name]) => name !== "alice" && name !== "carol");
  const avatars = Object.fromEntries(others.map(([name, id], i) => [id, name === "erin" ? "bm90IGEgcGljdHVyZQ==" : PICTURES[i % PICTURES.length]]));
  w.dashpay.avatars = { ...w.dashpay.avatars, ...avatars };
  w.loadContactPictures = on;
  return w;
}

/** DP3-07 D10: besides Bob's and Dave's, the newest payment from Marta (no relation to the main
 * identity), "Coffee refund" from Bob (a label over the contact), an older payment whose
 * counterparty's names have not landed yet, and the oldest payment, to Judy (hidden). */
function attributeMore(w: World) {
  attributePayments(w);
  const rows = w.wallets[0].rows;
  rows[0].counterparty = { identity: IDENTITY.marta, username: "marta", displayName: "Marta", avatarUrl: null };
  // The newest labelled receipt, "Coffee refund" (its text stays wallet.ts's sentinel).
  const refund = rows.find((r) => r.label && r.amount > 0)!;
  refund.counterparty = { identity: IDENTITY.bob, username: "bob", displayName: "Bob Builder", avatarUrl: null };
  w.dashpay.activity[IDENTITY.bob].unshift({ txid: refund.txid, direction: "received", amount: refund.amount, height: refund.blockHeight, timestamp: refund.timestamp });
  const nameless = rows.find((r) => r.txType === "recvWithAddress" && r.category === "received" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  nameless.counterparty = { identity: IDENTITY.mateo, username: null, displayName: null, avatarUrl: null };
  // The oldest plain payment went to Judy, a contact the user hid: listed, in the collapsed Hidden section.
  const hidden = rows.findLast((r) => r.txType === "sendToAddress" && !r.label && !r.counterparty && r.status.kind === "confirmed")!;
  hidden.counterparty = { identity: IDENTITY.judy, username: "judy", displayName: "Judy", avatarUrl: null };
  (w.dashpay.activity[IDENTITY.judy] ??= []).push({ txid: hidden.txid, direction: "sent", amount: Math.abs(hidden.amount) - (hidden.fee ?? 0), height: hidden.blockHeight, timestamp: hidden.timestamp });
}

function twoWallets(w: World): World {
  w.wallets.push(emptyWallet(SECOND_WALLET_ID, "Savings"));
  w.lock = unlocked([FIXTURE_WALLET_ID, SECOND_WALLET_ID]);
  w.phrases[SECOND_WALLET_ID] = SECOND_PHRASE;
  return w;
}

/** Three mixed coins (100.001, 10.0001 and 1.00001 DASH denominations), counted in the balances. */
function withCoinJoinCoins(w: World): World {
  const main = w.wallets[0];
  const { hex } = makeIds(prng(0xc01d));
  const mixed = [10_000_100_000, 1_000_010_000, 100_001_000].map((amount, i): Utxo => ({
    ...main.coins[0],
    outpoint: { txid: hex(64), vout: i },
    amount,
    label: null,
    coinjoinDenominated: true,
    coinjoinRounds: 4 + i,
  }));
  main.coins = [...main.coins, ...mixed];
  const sum = mixed.reduce((n, u) => n + u.amount, 0);
  const b = main.summary.balances!;
  main.summary = { ...main.summary, balances: { ...b, confirmed: b.confirmed + sum, total: b.total + sum, coinjoin: b.coinjoin + sum } };
  return w;
}

/** "alice" contested: her identity holds the temporary "alice007" while masternodes vote. */
function contestPending(w: World, endsAt: number | null): World {
  const world = registered();
  w.dashpay = {
    ...world,
    status: { kind: "contestPending", identity: IDENTITY.alice, label: "alice", endsAt },
    identities: world.identities.map((i) => ({ ...i, names: ["alice007"], mainName: "alice007" })),
  };
  return w;
}

/** "alice" registered, her summary patched (DP1-10). */
function withAlice(w: World, patch: Partial<IdentitySummary>): World {
  w.dashpay = registered();
  w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, ...patch }));
  return w;
}

/** "alice" registered with this credits state (DP1-10). */
function withCredits(w: World, patch: Partial<CreditsWorld>): World {
  w.dashpay = { ...registered(), credits: newCreditsWorld(patch) };
  return w;
}

/** A top-up lock of 0.05 Dash for "alice" (DP1-10). */
const topUpLock = (patch: Partial<PendingTopUp>): PendingTopUp => ({
  txid: "a1".repeat(32),
  identity: IDENTITY.alice,
  identityIndex: 0,
  duffs: 5_000_000,
  proved: false,
  consumptionUnknown: false,
  ...patch,
});

/** The contested world with `contest_status` reading `state` (DP1-10 R3–R5). */
function withContest(w: World, state: ContestStatus["state"]): World {
  const s = w.dashpay.status;
  if (s.kind !== "contestPending") return w;
  const contest: ContestStatus = {
    label: s.label,
    state,
    endsAt: s.endsAt,
    contenders: [
      { identity: IDENTITY.alice, votes: 31, isSelf: true },
      { identity: IDENTITY.mallory, votes: 40, isSelf: false },
    ],
    lockVotes: state.kind === "locked" ? 55 : 3,
    abstainVotes: 2,
    temporaryName: "alice007",
  };
  w.dashpay.credits = newCreditsWorld({ contest });
  return w;
}

/** The wallet's DashPay status set to `status`, no identity on file (DP1-08). The bring-up's
 * outcome says the same: a `startupIncomplete` status's own, read-only for a watch-only wallet
 * (Tools ▸ Information reads both, DP6-03). */
function withStatus(w: World, status: DashPayStatus): World {
  const d = noIdentity();
  if (status.kind === "startupIncomplete") d.startup = { ...d.startup, startup: status.startup };
  if (status.kind === "noIdentity" && status.reason === "watchOnly") d.startup = { ...d.startup, readOnly: true };
  w.dashpay = { ...d, status };
  return w;
}

/** DP6-03: `dashpay-registered` with the session's quorum source, the toggle and a sync report. */
function withSyncInfo(t: ThemeName | null, trust: World["trust"], lastPass: SyncPassReport | null = null): World {
  const w = base(t);
  w.dashpay = registered();
  w.trust = trust;
  if (lastPass) w.dashpay.lastPass = lastPass;
  return w;
}

/** A pass that ended 5 s before FIXTURE_NOW after 3 s. */
const pass = (found: Partial<SyncPassReport>): SyncPassReport => ({
  startedAt: secs(FIXTURE_NOW) - 8,
  finishedAt: secs(FIXTURE_NOW) - 5,
  newRequests: 0,
  newContacts: 0,
  newPayments: 0,
  failureCode: null,
  ...found,
});

/** Syncing: filters 60% through, the masternode list not started (65% in all). */
function syncing(): SyncStatus {
  const s = caughtUp();
  return {
    ...s,
    phases: s.phases.map((p) => {
      if (p.phase === "filters") return { ...p, currentHeight: Math.round(p.targetHeight! * 0.6), done: false };
      if (p.phase === "masternodes") return { ...p, currentHeight: 0, done: false };
      return p;
    }),
    activePhase: "filters",
    caughtUp: false,
  };
}

/** The `saved-links` scenario's links: a testnet payee and a contact's identity. */
const SAVED_PAYEE = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const SAVED_CONTACT = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";

const SCENARIOS = {
  /** The reference wallet: balance, a month of history, caught up, unlocked. */
  funded: (t) => base(t),
  /** A new wallet: no history, zero balance. */
  empty: (t) => base(t, "empty"),
  /** No wallet on the network yet, and no vault. */
  "no-wallet": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = { ...unlocked([]), state: "noVault", encrypted: false };
    return w;
  },
  /** A vault (passphrase `demo`) that holds no wallet yet: onboarding without a passphrase step. */
  "no-wallet-vault": (t) => {
    const w = base(t);
    w.wallets = [];
    w.phrases = {};
    w.lock = unlocked([]);
    return w;
  },
  /** The funded wallet with links opened before it existed (DP5-01): "Links you opened" at
   * launch, a payment request, a contact and an invitation. */
  "saved-links": (t) => {
    const w = base(t);
    const now = Math.floor(Date.now() / 1000);
    stashRoute(w.stash, { kind: "invitation", linkId: "" }, now - 3 * 86_400);
    stashRoute(w.stash, { kind: "addContact", identity: SAVED_CONTACT, username: "alice" }, now - 2 * 3600);
    stashRoute(w.stash, { kind: "pay", uri: { address: SAVED_PAYEE, amount: 25_000_000, label: "Coffee", message: null }, username: null }, now - 5 * 60);
    return w;
  },
  /** The funded wallet in a vault without a passphrase: Settings ▸ Security offers Encrypt (QT-111). */
  unencrypted: (t) => {
    const w = base(t);
    w.lock = { ...w.lock, state: "unencrypted", encrypted: false };
    return w;
  },
  /** The funded wallet behind the lock screen (passphrase `demo`). */
  locked: (t) => {
    const w = base(t);
    w.lock = { ...w.lock, state: "locked" };
    return w;
  },
  /** Three wrong passphrases: the lock screen waits a minute (IOS-012). */
  "locked-throttled": (t) => lockedAfter(base(t), 3),
  /** Eight wrong passphrases: only Forgot passphrase is left (IOS-012). */
  "locked-disabled": (t) => lockedAfter(base(t), 8),
  /** Two wallets: the toolbar's wallet picker. */
  "two-wallets": (t) => twoWallets(base(t)),
  /** Two wallets behind the lock screen: Forgot passphrase asks which one (IOS-014). */
  "locked-two-wallets": (t) => lockedAfter(twoWallets(base(t)), 0),
  /** 10,003 transactions: paging and render cost. */
  large: (t) => base(t, "large"),
  /** Hostile labels and names (U6): they must render as text. */
  xss: (t) => base(t, "xss"),
  "dashpay-no-identity": (t) => base(t),
  /** A username registration funded and waiting for its InstantSend lock. */
  "dashpay-registering": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.leases = [lease("9a0b1c2d", "registration", true)];
    return w;
  },
  /** "alice", with contacts in every relation and channel state and two requests waiting;
   * payments from Bob and to Dave attributed in the history (DP3-02). */
  "dashpay-registered": (t) => {
    const w = base(t);
    w.dashpay = registered();
    attributePayments(w);
    return w;
  },
  /** DP4-02: `dashpay-registered` with Alice's avatar published and verified. */
  "dashpay-avatar": (t) => withAvatar(t),
  /** DP4-03: `dashpay-avatar` with a picture for nearly everyone the lists show (contacts,
   * requests, history counterparties, the frequent strip, the bell, the search): Carol has
   * none (her initials), and Erin's thumbnail is not a PNG (the initials after it fails). */
  "dashpay-avatars": (t) => withPictures(t, true),
  /** DP4-03 D13: `dashpay-avatars` with "Load contact pictures" off: only Alice's own picture. */
  "dashpay-pictures-off": (t) => withPictures(t, false),
  /** DP4-03 D5: `dashpay-registered` with an upload host, so a picture from a file can be set. */
  "dashpay-avatar-upload": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), pictures: { upload: true, candidates: [], prepares: [] } };
    return w;
  },
  /** DP4-03: `dashpay-avatar` whose first profile save fails with `platform.unavailable`. */
  "dashpay-profile-save-error": (t) => {
    const w = withAvatar(t);
    w.dashpay.profileSaveFailures = 1;
    return w;
  },
  /** DP3-03b: `dashpay-registered` with "Include my username in payment requests" off: the
   * Receive QR and the requests are plain Core URIs. */
  "dashpay-username-off": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.wallets[0].receive.usernameInRequests = false;
    return w;
  },
  /** DP3-07: `dashpay-registered` with more attributed payments among the newest rows (D10). */
  "dashpay-history-contacts": (t) => {
    const w = base(t);
    w.dashpay = registered();
    attributeMore(w);
    return w;
  },
  /** "alice" contested: her identity holds the temporary "alice007" while masternodes vote. The
   * chip says "@alice · Pending"; Contacts, the bell and the badge stay (BL-153 D11). */
  "dashpay-contest-pending": (t) => contestPending(base(t), 1_900_000_000),
  // DP1-08: the Home card's other banner states (the status read failing is `fail=dashpay_status:none`).
  /** No identity while the wallet syncs: Join DashPay disabled, "Available after sync finishes". */
  "dashpay-waiting-for-sync": (t) => {
    const w = withStatus(base(t), { kind: "noIdentity", reason: "waitingForSync" });
    w.sync = syncing();
    return w;
  },
  /** A watch-only wallet: it cannot join, and the chip is hidden (DP1-08 D3). */
  "dashpay-watch-only": (t) => {
    const w = withStatus(base(t), { kind: "noIdentity", reason: "watchOnly" });
    w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
    return w;
  },
  /** DashPay's bring-up is looking for the wallet's identity. */
  "dashpay-starting": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "starting" }),
  /** No bring-up ran (a launch without sync). */
  "dashpay-not-started": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "notRun" }),
  /** The bring-up ran locked; unlocked for mixing only, the card offers the full unlock. */
  "dashpay-identity-unsettled": (t) => {
    const w = withStatus(base(t), { kind: "startupIncomplete", startup: "identityUnsettled" });
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** The bring-up could not finish its discovery. */
  "dashpay-look-failed": (t) => withStatus(base(t), { kind: "startupIncomplete", startup: "discoveryFailed" }),
  // DP6-03: Tools ▸ Information's DashPay card. `dashpay-registered` is its degraded mode (the
  // default quorum source, DEC-168); the states without an identity are the scenarios above.
  /** Quorum keys from SPV, a pass that found two requests and a contact, two contacts waiting for
   * an unlock: the card says Running. */
  "dashpay-info-spv": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false }, pass({ newRequests: 2, newContacts: 1 }));
    w.dashpay.startup = { ...w.dashpay.startup, contactAccountsPending: 2 };
    return w;
  },
  /** Before masternode sync: the trusted service's keys, results unverified. */
  "dashpay-info-fallback": (t) => withSyncInfo(t, { source: "trustedFallback", trustedOnly: false }),
  /** The developer toggle: the trusted service only. */
  "dashpay-info-trusted-only": (t) => withSyncInfo(t, { source: "trusted", trustedOnly: true }),
  /** The last pass failed with `platform.unavailable`. */
  "dashpay-info-pass-failed": (t) => withSyncInfo(t, { source: "spv", trustedOnly: false }, pass({ failureCode: "platform.unavailable" })),
  /** SPV stopped: the loops do not run, so Sync now has nothing to run. */
  "dashpay-info-stopped": (t) => {
    const w = withSyncInfo(t, { source: "spv", trustedOnly: false });
    w.sync = { ...w.sync, running: false, connectedPeers: 0 };
    return w;
  },
  /** A registration whose row is not listed (yet): the card's generic line. */
  "dashpay-registering-unread": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), registrations: [] };
    return w;
  },
  /** The registration waits for the full unlock (unlocked for mixing only). */
  "dashpay-registering-unlock": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = { ...w.dashpay.registrations[0], phase: "draft", waiting: "unlock", txid: null, holdsKey: false, running: false, fundsCommitted: false };
    w.dashpay.runs = [];
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** The registration stopped, retryably: not enough Dash for its lock. */
  "dashpay-registering-paused": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: null,
      txid: null,
      holdsKey: false,
      running: false,
      fundsCommitted: false,
      failure: { phase: "keysPrepared", code: "registration.funding_insufficient", retryable: true, needed: "3002260", available: "1000000" },
    };
    w.dashpay.runs = [];
    return w;
  },
  // DP1-09: a registration's screens (R2, R5, R8). Its run ended with the last launch.
  /** Funded, after a relaunch: no proof yet, so it reads `chainLock` with no key held, and waits for Continue (R2, D21). */
  "dashpay-registration-paused": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    return w;
  },
  /** Its asset lock is proved but no flow finished it: Finish transfers (R5). */
  "dashpay-registration-needs-finish": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    w.dashpay.registrations[0].waiting = "needsFinish";
    return w;
  },
  /** Failed for good after its funding went out: the recovery copy and Repair (R8). The status no
   * longer names it (banner.rs `in_progress`); the Join page finds it (D22). */
  "dashpay-registration-stopped": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [], status: { kind: "noIdentity", reason: null } };
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: null,
      holdsKey: false,
      running: false,
      failure: { phase: "fundingSent", code: "registration.islock_timeout", retryable: false, needed: null, available: null },
    };
    return w;
  },
  /** Failed (a retryable timeout) while its funding's outcome is unknown: the engine keeps the
   * wait of the step it still owns (`Awaiting`), so the page, Home and Tools ▸ Repair send it to
   * Resolve stuck funding (f1, Sol r1 P2). */
  "dashpay-registration-failed-unknown": (t) => {
    const w = base(t);
    w.dashpay = { ...registering(), runs: [] };
    w.dashpay.registrations[0] = {
      ...w.dashpay.registrations[0],
      phase: "failed",
      waiting: "fundingUnknown",
      holdsKey: false,
      running: false,
      fundsCommitted: true,
      failure: { phase: "keysPrepared", code: "platform.timeout", retryable: true, needed: null, available: null },
    };
    return w;
  },
  /** "alice" contested, the vote's end not read yet. */
  "dashpay-contest-pending-no-end": (t) => contestPending(base(t), null),
  /** Alice without a username (its name step failed): Finish username registration; My QR has no link (DP2-08 D12). */
  "dashpay-no-username": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null })) };
    return w;
  },
  /** A sync pass holds Alice's names (S18): no card, the chip says "Updating…". */
  "dashpay-names-updating": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesUpdating: true })) };
    return w;
  },
  /** A restore found Alice; the engine's names pass that fetches her names has not ended (D23):
   * no card, the chip says "Updating…". */
  "dashpay-names-pending": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesCheck: "pending" as const })) };
    return w;
  },
  /** That names pass failed (D23): Alice may own a name, so no card and a neutral chip. */
  "dashpay-names-unknown": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, identities: world.identities.map((i) => ({ ...i, names: [], mainName: null, namesCheck: "failed" as const })) };
    return w;
  },
  /** Tools ▸ Repair: the registration's funding broadcast's outcome is unknown (E0-04.P2b). */
  "tools-stuck-funding": (t) => {
    const w = base(t);
    w.dashpay = registering();
    w.dashpay.registrations[0].waiting = "fundingUnknown";
    return w;
  },
  /** Tools ▸ Information: every node warning; the banner shows the most severe (QT-040). */
  "tools-warnings": (t) => {
    const w = base(t);
    w.tools.warnings = [
      { code: "prereleaseBuild", detail: "" },
      { code: "syncStalled", detail: "" },
      { code: "uncleanShutdown", detail: "" },
    ];
    return w;
  },
  /** The funded wallet watch-only: Pay's confirm sheet offers Create Unsigned only (U-05 D18). */
  "watch-only": (t) => {
    const w = base(t);
    w.wallets[0].summary = { ...w.wallets[0].summary, watchOnly: true };
    return w;
  },
  /** Coin Selection: CoinJoin denominations beside the plain coins, hidden by default (QT-071). */
  "tools-coinjoin": (t) => withCoinJoinCoins(base(t)),
  /** "alice" before her first contact: the Contacts empty state (DP2-07). */
  "dashpay-contacts-empty": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contacts: [], requestTimes: {}, journal: [] };
    return w;
  },
  /** A sync pass holds the wallet's state before any contacts snapshot: "Updating contacts…". */
  "dashpay-contacts-updating": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contactsUpdating: "noSnapshot" };
    return w;
  },
  /** A sync pass holds the wallet's state after a snapshot: the rows, under "Updating contacts…". */
  "dashpay-contacts-refreshing": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), contactsUpdating: "snapshot" };
    return w;
  },
  /** Unlocked for mixing only, Carol's channel still setting up: "Unlock to finish setting up". */
  "dashpay-contacts-mixing-only": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.lock = { ...w.lock, state: "unlockedMixingOnly" };
    return w;
  },
  /** "alice" registered before DashPay keys existed: the Enable DashPay banner (DP2-09 D10). */
  "dashpay-no-keys": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, hasDashpayKeys: false }));
    return w;
  },
  /** "alice" with fewer credits than one contact request: Accept is refused (DP2-09 D4). */
  "dashpay-low-credits": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, balance: "4000000", creditLevel: "empty" }));
    return w;
  },
  // DP1-10: My Profile's credits block, top-ups and the request card.
  /** "alice" at or below the low threshold: the low warning and Home's card (C2, S19). */
  "dashpay-credits-low": (t) => withAlice(base(t), { balance: "1000000000", creditLevel: "low" }),
  /** The balance not read yet (C4). */
  "dashpay-credits-unknown": (t) => withAlice(base(t), { balance: null, balanceAsOf: null, balanceStale: true, creditLevel: null }),
  /** The balance may be behind (C5). */
  "dashpay-credits-stale": (t) => withAlice(base(t), { balanceStale: true }),
  /** A top-up of 0.05 Dash waiting for its proof (C6). */
  "dashpay-top-up-pending": (t) => withCredits(base(t), { pending: [topUpLock({ proved: false })] }),
  /** A top-up lock Platform answered "already consumed" for, proved (C6). */
  "dashpay-top-up-unknown": (t) => withCredits(base(t), { pending: [topUpLock({ proved: true, consumptionUnknown: true })] }),
  /** A top-up funding whose outcome is unknown: Repair resolves it (C7, D16). */
  "dashpay-top-up-stuck": (t) =>
    withCredits(base(t), { stuck: [{ step: topUpStep(0, 1), identityIndex: 0, identity: IDENTITY.alice, state: "maybeSent" }] }),
  /** The next top-up's funding outcome is unknown: the sheet routes to Repair (T8, D16, D22). */
  "dashpay-top-up-lost": (t) => withCredits(base(t), { next: "broadcastUnknown" }),
  /** "alice" contested with no temporary name: Create instant username (R1, D13). */
  "dashpay-contest-no-temporary": (t) => {
    const w = contestPending(base(t), 1_900_000_000);
    w.dashpay.identities = w.dashpay.identities.map((i) => ({ ...i, names: [], mainName: null }));
    return w;
  },
  /** The contest read before the status moved on: won (R3), lost to another user (R4), blocked (R5). */
  "dashpay-contest-won": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "won" }),
  "dashpay-contest-lost": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "lost", winner: IDENTITY.mallory }),
  "dashpay-contest-locked": (t) => withContest(contestPending(base(t), 1_900_000_000), { kind: "locked" }),
  /** "alice" with nothing in the bell: no request waiting, no event (DP2-09 D12). */
  "dashpay-bell-empty": (t) => {
    const w = base(t);
    const world = registered();
    w.dashpay = { ...world, contacts: world.contacts.filter((c) => c.summary.relation !== "pendingIncoming"), journal: [] };
    return w;
  },
  /** Two wallets, each "alice" with her own contacts and unread events: the bell is the window
   * wallet's (DP2-09 D27). */
  "dashpay-two-wallets": (t) => {
    const w = twoWallets(base(t));
    w.dashpay = registered();
    w.dashpayOf = { [SECOND_WALLET_ID]: registered() };
    return w;
  },
  /** DAPI does not answer user searches: the network section fails with Try again. */
  "dashpay-search-offline": (t) => {
    const w = base(t);
    w.dashpay = { ...registered(), searchOffline: true };
    return w;
  },
  /** macOS: no screen picker; Add Contact points to the system's copy-a-region shortcut (D10). */
  "dashpay-screen-shortcut": (t) => {
    const w = base(t);
    w.dashpay = registered();
    w.scan.mode = "copyShortcut";
    return w;
  },
  /** Locking cuts more flows than the report lists, some pending (E0-13 D3): unresolved for good. */
  "lock-report-omitted": (t) => omittedPending(base(t)),
  /** The report omits flows, artifacts and wallets that its order shows are not pending. */
  "lock-report-omitted-settles": (t) => omittedSettled(base(t)),
  /** Locking cuts flows; the report lists them all, some still pending. */
  "lock-report-pending": (t) => listedFlows(base(t)),
  /** Locking cuts flows that all ended: sent or cancelled. */
  "lock-report-resolved": (t) => resolvedFlows(base(t)),
  /** A cut call reported under its label (no id to check): unresolved for good. */
  "lock-report-call": (t) => unnamedCall(base(t)),
  // CoinJoin (U-06): coinjoin.ts.
  ...coinjoinScenarios(base),
  // Governance (MG-09): governance.ts.
  ...governanceScenarios(base),
} satisfies Record<string, (theme: ThemeName | null) => World>;

export type ScenarioName = keyof typeof SCENARIOS;
export const SCENARIO_NAMES = Object.keys(SCENARIOS) as ScenarioName[];
export const DEFAULT_SCENARIO: ScenarioName = "funded";

export function isScenario(s: string): s is ScenarioName {
  return Object.hasOwn(SCENARIOS, s);
}

export function buildWorld(name: ScenarioName, theme: ThemeName | null = null): World {
  return SCENARIOS[name](theme);
}
